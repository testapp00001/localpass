// V26: the service worker restricts storage.local to trusted contexts before it reads or writes
// anything (content scripts can read storage.local by default), and fails closed if it cannot.

import { beforeEach, describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { BrokerClient } from "../src/background/broker";
import { IDBFactory } from "fake-indexeddb";
import { indexedDbArea, local } from "../src/background/storage";
import { type FakeChrome, installChrome, until } from "./chrome";
import { honestApp, seedCredentials } from "./fakeapp";

let fake: FakeChrome;
beforeEach(() => {
  fake = installChrome();
});

describe("storage access level", () => {
  test("the first thing done to storage.local is setAccessLevel(TRUSTED_CONTEXTS), and reads wait for it", async () => {
    seedCredentials(fake);
    fake.server = honestApp();
    const broker = new BrokerClient(() => {});
    await broker.connect();
    await until(() => broker.state.phase === "ready");
    expect(fake.local.log[0]).toBe("setAccessLevel:TRUSTED_CONTEXTS");
    expect(fake.local.log.indexOf("get")).toBeGreaterThan(0);
    expect(fake.local.accessLevel).toBe("TRUSTED_CONTEXTS");
  });

  test("the level is set once per storage area, however many reads follow", async () => {
    await Promise.all([local(), local(), local()]);
    expect(fake.local.log.filter((l) => l.startsWith("setAccessLevel"))).toHaveLength(1);
  });

  test("when the level cannot be set nothing is read, written or connected", async () => {
    seedCredentials(fake);
    fake.local.failAccessLevel = true;
    fake.server = honestApp();
    const broker = new BrokerClient(() => {});
    await broker.connect();
    expect(broker.state.phase).toBe("error");
    expect(fake.ports).toHaveLength(0);
    expect(fake.local.log).toEqual(["setAccessLevel:TRUSTED_CONTEXTS"]);
  });

  test("pairing writes the secret only after the level is set", async () => {
    fake.server = honestApp();
    const broker = new BrokerClient(() => {});
    await broker.connect(true);
    await until(() => broker.state.phase === "ready");
    expect(fake.local.log.indexOf("setAccessLevel:TRUSTED_CONTEXTS")).toBe(0);
    expect(fake.local.log).toContain("set");
    expect(fake.local.data.secret).toBeTruthy();
  });
});

describe("content scripts never use storage.local", () => {
  // A content script cannot read storage.local any more, so the code that runs in pages must not
  // try (the passkey switch is asked of the service worker instead).
  test("no file under src/content touches chrome.storage", () => {
    const dir = join(import.meta.dir, "../src/content");
    for (const name of readdirSync(dir)) {
      expect(readFileSync(join(dir, name), "utf8"), name).not.toMatch(/chrome\.storage|\bstorage\.(local|session|sync)/);
    }
  });

  test("the service worker and the popup only reach storage.local through local()", () => {
    for (const sub of ["background", "popup", "shared"]) {
      const dir = join(import.meta.dir, "../src", sub);
      for (const name of readdirSync(dir)) {
        if (name === "storage.ts") continue;
        // No other route to the data either: in Firefox it is not in storage.local at all.
        expect(readFileSync(join(dir, name), "utf8"), `${sub}/${name}`).not.toMatch(/chrome\.storage\.local|storage\.onChanged/);
      }
    }
  });
});

describe("Firefox: the persistent area is IndexedDB, never storage.local", () => {
  test("get, set and remove, and a value survives reopening the database", async () => {
    const factory = new IDBFactory();
    const first = await indexedDbArea(factory);
    expect(await first.get(["clientId", "secret"])).toEqual({});
    await first.set({ clientId: "c1", secret: "s1", neverSave: ["https://a.example"] });
    expect(await first.get(["clientId", "secret"])).toEqual({ clientId: "c1", secret: "s1" });
    const second = await indexedDbArea(factory);
    expect(await second.get("neverSave")).toEqual({ neverSave: ["https://a.example"] });
    await second.remove(["clientId", "secret"]);
    expect(await first.get(["clientId", "secret", "neverSave"])).toEqual({ neverSave: ["https://a.example"] });
  });

  test("it never touches chrome.storage", async () => {
    const area = await indexedDbArea(new IDBFactory());
    await area.set({ secret: "s" });
    await area.get("secret");
    await area.remove("secret");
    expect(fake.local.log).toEqual([]);
    expect(fake.session.log).toEqual([]);
  });

  test("a database that cannot be opened is an error, not a fallback to storage.local", async () => {
    const broken = { open: () => { throw new Error("no IndexedDB"); } } as unknown as IDBFactory;
    await expect(indexedDbArea(broken)).rejects.toThrow("no IndexedDB");
    expect(fake.local.log).toEqual([]);
  });

  test("the build constant picks the backend", () => {
    // Only the Firefox build (`__FIREFOX__`) may use IndexedDB, and only it may skip setAccessLevel.
    const source = readFileSync(join(import.meta.dir, "../src/background/storage.ts"), "utf8");
    expect(source).toMatch(/if \(!__FIREFOX__\) return chromeArea\(\)/);
  });
});

describe("Firefox: pairing through the broker client", () => {
  test("the pairing secret goes to IndexedDB and storage.local is never touched", async () => {
    const g = globalThis as unknown as { __FIREFOX__: boolean; indexedDB: unknown };
    const saved = { flag: g.__FIREFOX__, idb: g.indexedDB };
    g.__FIREFOX__ = true;
    g.indexedDB = new IDBFactory();
    try {
      fake.server = honestApp();
      const broker = new BrokerClient(() => {});
      await broker.connect(true);
      await until(() => broker.state.phase === "ready");
      expect(fake.local.log).toEqual([]);
      expect(fake.local.data.secret).toBeUndefined();
      const stored = await (await local()).get(["clientId", "secret"]);
      expect(typeof stored.secret).toBe("string");
    } finally {
      g.__FIREFOX__ = saved.flag;
      g.indexedDB = saved.idb;
    }
  });
});
