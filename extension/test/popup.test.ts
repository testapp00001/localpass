// The popup: the list of all logins is fetched once on an explicit click (every fetch is a Windows
// Hello prompt in the app) and filtered here; a "no" from the user is shown quietly; a browser the
// app forgot is asked to pair again.

import { beforeEach, describe, expect, test } from "bun:test";
import { type Json, installChrome, until } from "./chrome";

const TAB = { id: 5, origin: "https://a.example", host: "a.example", title: "" };
const ITEMS = [
  { id: "1", kind: "login", title: "Shop", subtitle: "alice", url: "https://shop.example", favorite: false, hasPassword: true, hasTotp: false, trashed: false },
  { id: "2", kind: "login", title: "Bank", subtitle: "bob", url: "https://bank.example", favorite: false, hasPassword: true, hasTotp: false, trashed: false },
  { id: "3", kind: "login", title: "Old", subtitle: "carol", url: null, favorite: false, hasPassword: true, hasTotp: false, trashed: true },
];

let asked: Json[] = [];
let alerts: string[] = [];
let answers: Record<string, (message: Json) => unknown> = {};
let n = 0;

async function boot(state: Json = {}, overrides: typeof answers = {}) {
  asked = [];
  alerts = [];
  document.body.innerHTML = `<div id="site"></div><div id="main"></div><input type="checkbox" id="privacy"><p id="privacy-note"></p><input type="checkbox" id="passkeys">`;
  const fake = installChrome();
  void fake;
  answers = {
    state: () => ({ ok: true, data: { broker: { phase: "ready" }, tab: TAB, logins: [], vault: "unlocked", ...state } }),
    getPrivacy: () => ({ ok: true, data: { disabled: false, control: "controllable_by_this_extension" } }),
    cachedLogins: () => ({ ok: true, data: null }),
    loadLogins: () => ({ ok: true, data: ITEMS }),
    fillTab: () => ({ ok: true, data: null }),
    ...overrides,
  };
  const runtime = (globalThis as unknown as { chrome: { runtime: Record<string, unknown> } }).chrome.runtime;
  runtime.sendMessage = async (message: Json) => {
    asked.push(message);
    return answers[message.kind as string]?.(message) ?? { ok: true, data: null };
  };
  (globalThis as unknown as Record<string, unknown>).alert = (m: string) => alerts.push(m);
  (globalThis as unknown as Record<string, unknown>).confirm = () => true;
  await import(`../src/popup/index.ts?${++n}`);
}

const main = () => document.getElementById("main")!;
const button = (label: string) => [...main().querySelectorAll("button")].find((b) => b.textContent === label) as HTMLButtonElement | undefined;
const count = (kind: string) => asked.filter((m) => m.kind === kind).length;
const rows = () => [...main().querySelectorAll("ul.list li")].map((li) => li.textContent ?? "");

describe("searching all logins", () => {
  beforeEach(() => boot());

  test("the popup explains the Windows Hello prompt and asks nothing until the button is pressed", async () => {
    await until(() => !!button("Show all my logins"));
    expect(main().textContent).toContain("Windows Hello");
    expect(count("loadLogins")).toBe(0);
    expect(main().querySelector("input[type=search]")).toBeNull();
  });

  test("one click fetches the list once; typing only filters it", async () => {
    await until(() => !!button("Show all my logins"));
    button("Show all my logins")!.click();
    await until(() => !!main().querySelector("input[type=search]"));
    expect(count("loadLogins")).toBe(1);
    // Trashed items are not listed.
    expect(rows().join("|")).toContain("Shop");
    expect(rows().join("|")).toContain("Bank");
    expect(rows().join("|")).not.toContain("Old");
    const input = main().querySelector("input[type=search]") as HTMLInputElement;
    for (const text of ["b", "ba", "ban", "bank", "bank.example"]) {
      input.value = text;
      input.dispatchEvent(new Event("input"));
    }
    await new Promise((r) => setTimeout(r, 300));
    expect(count("loadLogins")).toBe(1);
    expect(count("search")).toBe(0);
    expect(rows()).toHaveLength(1);
    expect(rows()[0]).toContain("Bank");
    input.value = "zzz";
    input.dispatchEvent(new Event("input"));
    expect(rows()).toEqual(["Nothing found"]);
  });

  test("a list asked for a moment ago (the popup closed during Hello) is shown without asking again", async () => {
    await boot({}, { cachedLogins: () => ({ ok: true, data: ITEMS }) });
    await until(() => !!main().querySelector("input[type=search]"));
    expect(count("loadLogins")).toBe(0);
    expect(rows()).toHaveLength(2);
  });

  test("a full list says that it was cut", async () => {
    const many = Array.from({ length: 200 }, (_, i) => ({ ...ITEMS[0]!, id: `x${i}`, title: `Site ${i}` }));
    await boot({}, { loadLogins: () => ({ ok: true, data: many }) });
    await until(() => !!button("Show all my logins"));
    button("Show all my logins")!.click();
    await until(() => !!main().querySelector("input[type=search]"));
    expect(main().textContent).toContain("first 200");
  });

  test("saying no to Windows Hello shows a quiet note, not an alert, and the button stays", async () => {
    await boot({}, { loadLogins: () => ({ ok: false, code: "denied", message: "The request was not approved" }) });
    await until(() => !!button("Show all my logins"));
    button("Show all my logins")!.click();
    await until(() => main().textContent!.includes("Not shown"));
    expect(alerts).toEqual([]);
    expect(button("Show all my logins")).toBeDefined();
  });

  test("another failure is shown as an error", async () => {
    await boot({}, { loadLogins: () => ({ ok: false, code: "tooMany", message: "LocalPass is busy" }) });
    await until(() => !!button("Show all my logins"));
    button("Show all my logins")!.click();
    await until(() => main().textContent!.includes("LocalPass is busy"));
  });

  test("filling a login for another site asks for the user's Hello, and a no is quiet", async () => {
    await boot({}, { fillTab: () => ({ ok: false, code: "denied", message: "The request was not approved" }) });
    await until(() => !!button("Show all my logins"));
    button("Show all my logins")!.click();
    await until(() => !!button("Fill"));
    button("Fill")!.click();
    await until(() => main().textContent!.includes("Not filled"));
    expect(alerts).toEqual([]);
    expect(asked.find((m) => m.kind === "fillTab")).toMatchObject({ force: true, tabId: 5 });
  });
});

describe("a browser the app forgot", () => {
  test("asks to pair again instead of showing the first-time text", async () => {
    await boot({ broker: { phase: "unpaired", forgotten: true } });
    await until(() => main().textContent!.includes("no longer knows this browser"));
    expect(button("Connect again")).toBeDefined();
    expect(main().textContent).not.toContain("Link this browser");
  });

  test("a new browser sees the normal first-time text", async () => {
    await boot({ broker: { phase: "unpaired" } });
    await until(() => !!button("Connect"));
    expect(main().textContent).toContain("Link this browser");
  });
});
