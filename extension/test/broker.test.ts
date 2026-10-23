// The handshake state machine (V1), the request envelope (V17) and how the connection recovers
// when the app closes it or forgets this browser, against a fake native-messaging port.

import { beforeEach, describe, expect, test } from "bun:test";
import { randomBytes } from "node:crypto";
import { BrokerClient } from "../src/background/broker";
import { toBase64Url } from "../src/shared/crypto";
import { BrokerError } from "../src/shared/protocol";
import { type FakeChrome, type FakePort, type Json, installChrome, sleep, until } from "./chrome";
import { AppError, honestApp, seedCredentials } from "./fakeapp";

let fake: FakeChrome;
let broker: BrokerClient;
/** The last server proof an honest app produced, so a test can replay it. */
let lastServerProof = "";

const honest = (answer?: (request: Json) => unknown) => honestApp({ answer, onProof: (p) => (lastServerProof = p) });

/** An app that behaves honestly except for what `change` does to each message it receives. */
function honestExcept(change: (message: Json, port: FakePort, normal: () => void) => void) {
  return (port: FakePort) => {
    honest()(port);
    const normal = port.onSend!;
    port.onSend = (m, p) => change(m, p, () => normal(m, p));
  };
}

const rand = () => toBase64Url(new Uint8Array(randomBytes(32)));

beforeEach(() => {
  fake = installChrome();
  lastServerProof = "";
  broker = new BrokerClient(() => {});
});

describe("handshake", () => {
  test("a known client proves itself and checks the app's proof", async () => {
    seedCredentials(fake);
    fake.server = honest();
    await broker.connect();
    await until(() => broker.state.phase === "ready");
    expect(await broker.request("status")).toEqual({ op: "status" });
  });

  test("an app that sends ready with a wrong proof is refused and nothing is sent to it", async () => {
    seedCredentials(fake);
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "challenge", nonce: rand() });
        else if (m.type === "auth") port.emit({ type: "ready", status: "unlocked", serverProof: rand(), appVersion: "x" });
      };
    };
    await broker.connect();
    await until(() => broker.state.phase === "untrusted");
    const port = fake.ports[0]!;
    await expect(broker.request("saveLogin", { password: "hunter2" })).rejects.toMatchObject({ code: "untrusted" });
    expect(port.requests).toEqual([]);
    expect(port.disconnected).toBe(true);
  });

  test("ready without any challenge (a fresh worker meets an impostor) is refused", async () => {
    seedCredentials(fake);
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "ready", status: "unlocked", serverProof: "AAAA", appVersion: "x" });
      };
    };
    await broker.connect();
    await until(() => broker.state.phase === "untrusted");
    await expect(broker.request("fillLogin", { itemId: "i" })).rejects.toMatchObject({ code: "untrusted" });
    expect(fake.ports[0]!.requests).toEqual([]);
  });

  test("ready when this browser has no credentials is refused", async () => {
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "ready", status: "unlocked", serverProof: "AAAA", appVersion: "x" });
      };
    };
    await broker.connect();
    await until(() => broker.state.phase === "untrusted");
  });

  test("a proof from an earlier connection does not work on the next one", async () => {
    seedCredentials(fake);
    fake.server = honest();
    await broker.connect();
    await until(() => broker.state.phase === "ready");
    const old = lastServerProof;
    fake.ports[0]!.serverClose();
    expect(broker.state.phase).toBe("disconnected");
    // The next peer replays the old proof straight away...
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "ready", status: "unlocked", serverProof: old, appVersion: "x" });
      };
    };
    await broker.connect();
    await until(() => broker.state.phase === "untrusted");
    // ...or after a challenge of its own.
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "challenge", nonce: rand() });
        else if (m.type === "auth") port.emit({ type: "ready", status: "unlocked", serverProof: old, appVersion: "x" });
      };
    };
    await broker.connect();
    await until(() => fake.ports.length === 3 && broker.state.phase === "untrusted" && fake.ports[2]!.sent.length >= 2);
    expect(fake.ports[2]!.requests).toEqual([]);
  });

  test("malformed base64 in the challenge or the proof is contained and refused", async () => {
    seedCredentials(fake);
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "challenge", nonce: "***not base64***" });
      };
    };
    await broker.connect();
    await until(() => broker.state.phase === "untrusted");

    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "challenge", nonce: rand() });
        else if (m.type === "auth") port.emit({ type: "ready", status: "unlocked", serverProof: "***", appVersion: "x" });
      };
    };
    await broker.connect();
    await until(() => fake.ports.length === 2 && broker.state.phase === "untrusted" && fake.ports[1]!.sent.length >= 2);
    expect(fake.ports[1]!.requests).toEqual([]);
  });

  test("paired from a peer we never asked to pair is refused and the stored secret stays", async () => {
    seedCredentials(fake);
    const before = { ...fake.local.data };
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "paired", clientId: "evil", secret: rand(), status: "unlocked" });
      };
    };
    await broker.connect();
    await until(() => broker.state.phase === "untrusted");
    expect(fake.local.data).toEqual(before);
  });

  test("an unpaired answer to a known client does not delete the credentials and does not pair by itself", async () => {
    seedCredentials(fake);
    const before = { ...fake.local.data };
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.emit({ type: "unpaired" });
      };
    };
    await broker.connect();
    await until(() => broker.state.phase === "unpaired");
    expect(fake.local.data).toEqual(before);
    expect(broker.state.forgotten).toBe(true);
    await sleep(30);
    expect(fake.ports[0]!.sent.some((m) => m.type === "pair")).toBe(false);
    // Asking for a request does not loop through reconnects either.
    await expect(broker.request("status")).rejects.toMatchObject({ code: "unpaired" });
    expect(fake.ports.length).toBe(1);
  });

  test("the user's Connect pairs, and the new credentials replace the old ones", async () => {
    seedCredentials(fake);
    // The app no longer knows this client: it asks to pair.
    fake.server = honestExcept((m, p, normal) => (m.type === "hello" ? p.emit({ type: "unpaired" }) : normal()));
    await broker.connect(true);
    await until(() => broker.state.phase === "ready");
    expect(fake.local.data.clientId).toBe("new-client");
  });

  test("a new browser is paired only when asked to", async () => {
    fake.server = honest();
    await broker.connect();
    await until(() => broker.state.phase === "unpaired");
    expect(broker.state.forgotten).toBeFalsy();
    expect(fake.ports[0]!.sent.some((m) => m.type === "pair")).toBe(false);
    await broker.connect(true);
    await until(() => broker.state.phase === "ready");
    expect(fake.local.data.clientId).toBe("new-client");
  });

  test("a pairing request left over from a failed Connect is not sent on a later automatic connection", async () => {
    fake.server = (port) => {
      port.onSend = (m) => {
        if (m.type === "hello") port.serverClose(); // the app is not there
      };
    };
    await broker.connect(true);
    await until(() => broker.state.phase === "disconnected" || broker.state.phase === "appNotRunning");
    fake.server = honest();
    await broker.connect();
    await until(() => broker.state.phase === "unpaired");
    await sleep(30);
    expect(fake.ports[1]!.sent.some((m) => m.type === "pair")).toBe(false);
  });
});

describe("requests", () => {
  test("a parameter named id or op cannot replace the envelope", async () => {
    seedCredentials(fake);
    fake.server = honest();
    await broker.request("findLogins", { id: 999, op: "searchItems", origin: "https://a.example" });
    const sent = fake.ports[0]!.requests[0]!;
    expect(sent.op).toBe("findLogins");
    expect(sent.id).toBe(1);
    expect(sent.origin).toBe("https://a.example");
  });

  test("answers for ids nobody asked about are ignored", async () => {
    seedCredentials(fake);
    fake.server = honest();
    await broker.connect();
    await until(() => broker.state.phase === "ready");
    fake.ports[0]!.emit({ id: 4242, ok: true, result: "late" });
    fake.ports[0]!.emit({ id: "x", ok: false });
    expect(await broker.request("status")).toEqual({ op: "status" });
  });

  test("a request that would not fit the helper's frame is refused before it is sent", async () => {
    seedCredentials(fake);
    fake.server = honest();
    await expect(broker.request("passkeyCreate", { exclude: ["x".repeat(1_100_000)] })).rejects.toMatchObject({ code: "tooLarge" });
    expect(fake.ports[0]!.requests).toEqual([]);
  });

  test("error codes from the app reach the caller unchanged", async () => {
    seedCredentials(fake);
    for (const code of ["tooLarge", "tooMany", "denied"]) {
      fake.server = honest(() => new AppError(code, `m-${code}`));
      broker = new BrokerClient(() => {});
      const error = (await broker.request("status").catch((e: unknown) => e)) as BrokerError;
      expect(error).toBeInstanceOf(BrokerError);
      expect(error.code).toBe(code);
      expect(error.message).toBe(`m-${code}`);
    }
  });
});

describe("recovering", () => {
  test("a connection the app closed (idle, or a restart) is reopened by the next request", async () => {
    seedCredentials(fake);
    fake.server = honest();
    await broker.request("status");
    fake.ports[0]!.emit({ type: "hostError", code: "appClosed", message: "LocalPass closed the connection" });
    fake.ports[0]!.serverClose();
    expect(broker.state.phase).toBe("disconnected");
    expect(await broker.request("status")).toEqual({ op: "status" });
    expect(fake.ports.length).toBe(2);
  });

  test("the app's closing notice alone is enough: the next request reconnects even if the port stays open", async () => {
    seedCredentials(fake);
    fake.server = honest();
    await broker.request("status");
    fake.ports[0]!.emit({ type: "hostError", code: "appClosed", message: "LocalPass closed the connection" });
    await until(() => broker.state.phase === "disconnected");
    expect(await broker.request("status")).toEqual({ op: "status" });
    expect(fake.ports.length).toBe(2);
  });

  test("a request in flight when the connection drops fails at once, not after the time-out", async () => {
    seedCredentials(fake);
    fake.server = honestExcept((m, p, normal) => ("op" in m ? p.serverClose() : normal()));
    const started = Date.now();
    await expect(broker.request("status")).rejects.toMatchObject({ code: "disconnected" });
    expect(Date.now() - started).toBeLessThan(1000);
  });

  test("a request sent while the port is gone fails at once", async () => {
    seedCredentials(fake);
    fake.server = honest();
    await broker.request("status");
    fake.ports[0]!.disconnected = true; // closed without a notification yet
    const result = await Promise.race([broker.request("status").then(() => "ok", (e: BrokerError) => e.code), sleep(1500).then(() => "hung")]);
    expect(result).not.toBe("hung");
  });

  test("a client the app forgot while connected: unpaired on a request clears the credentials and asks to pair again", async () => {
    seedCredentials(fake);
    fake.server = honestExcept((m, p, normal) => {
      if ("op" in m) {
        p.emit({ id: m.id, ok: false, error: { code: "unpaired", message: "this browser was removed" } });
        p.serverClose();
      } else normal();
    });
    await expect(broker.request("findLogins", { origin: "https://a.example" })).rejects.toMatchObject({ code: "unpaired" });
    expect(broker.state.phase).toBe("unpaired");
    expect(broker.state.forgotten).toBe(true);
    await until(() => !("secret" in fake.local.data), 1000, "the credentials to be removed");
    // The next attempt does not loop: it connects once, hears unpaired, and stops there.
    fake.server = honest();
    await expect(broker.request("status")).rejects.toMatchObject({ code: "unpaired" });
    await expect(broker.request("status")).rejects.toMatchObject({ code: "unpaired" });
    expect(fake.ports.length).toBe(2);
  });
});
