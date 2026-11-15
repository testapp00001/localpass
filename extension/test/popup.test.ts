// The popup: the list of all logins is fetched once on an explicit click (every fetch is a Windows
// Hello prompt in the app) and filtered here; a "no" from the user is shown quietly; a browser the
// app forgot is asked to pair again.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { type Json, FakePort, installChrome, sleep, until } from "./chrome";
import { timing } from "../src/decide/timing";

const TAB = { id: 5, origin: "https://a.example", host: "a.example", title: "" };
const ITEMS = [
  { id: "1", kind: "login", title: "Shop", subtitle: "alice", url: "https://shop.example", favorite: false, hasPassword: true, hasTotp: false, trashed: false },
  { id: "2", kind: "login", title: "Bank", subtitle: "bob", url: "https://bank.example", favorite: false, hasPassword: true, hasTotp: false, trashed: false },
  { id: "3", kind: "login", title: "Old", subtitle: "carol", url: null, favorite: false, hasPassword: true, hasTotp: false, trashed: true },
];

let asked: Json[] = [];
/** `alert`/`confirm` are gone from the popup: any call is recorded here and fails the test. */
let dialogs: string[] = [];
/** The popup's own port to the background, and its name. */
let popupPort: FakePort | undefined;
let portName = "";
let noted = false;
let fakeChrome: ReturnType<typeof installChrome>;
let answers: Record<string, (message: Json) => unknown> = {};
let n = 0;

/**
 * Who opened the popup, as the background tells it on the popup's port: "user" (the note arrives at once), "late"
 * (the note arrives after the first view), "background" (an own `openPopup`: no note), or another note.
 */
type Opener = "user" | "late" | "background" | { note: unknown };

const happy = () => (window as unknown as { happyDOM: { setURL(u: string): void } }).happyDOM;
/** The address before a test gave the popup its own (`search`), restored after each test. */
let savedUrl: string | undefined;

async function boot(state: Json = {}, overrides: typeof answers = {}, os = "win", opener: Opener = "user", search = "") {
  if (search) {
    savedUrl ??= window.location.href;
    happy().setURL(`chrome-extension://khlgdnhadbjfghhonaohnhdmedgnenlc/popup.html${search}`);
  }
  asked = [];
  dialogs = [];
  popupPort = undefined;
  portName = "";
  noted = false;
  document.body.innerHTML = `<div id="site"></div><div id="main"></div><input type="checkbox" id="privacy"><p id="privacy-note"></p><input type="checkbox" id="passkeys">`;
  const fake = installChrome();
  fakeChrome = fake;
  fake.os = os;
  const c = (globalThis as unknown as { chrome: Record<string, Record<string, unknown>> }).chrome;
  c.windows = { ...(c.windows ?? {}), getCurrent: async () => ({ id: 20 }) };
  c.runtime!.connect = ({ name }: { name: string }) => {
    portName = name;
    const port = new FakePort();
    popupPort = port;
    if (opener === "user") setTimeout(() => ((noted = true), port.emit({ userOpened: true })), 0);
    else if (typeof opener === "object") setTimeout(() => port.emit(opener.note), 0);
    return port;
  };
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
  for (const name of ["alert", "confirm", "prompt"]) {
    (globalThis as unknown as Record<string, unknown>)[name] = (m: string) => {
      dialogs.push(`${name}: ${m}`);
      throw new Error(`the popup called ${name}`);
    };
  }
  await import(`../src/popup/index.ts?${++n}`);
  // A person cannot click within the few milliseconds the background takes to answer.
  if (opener === "user" && !search) await until(() => noted, 1000, "the background's note");
}

/** Every popup the tests import listens on the shared window and document: removed after each test, so an earlier popup's hold cannot block a later test's click. */
const listeners: [EventTarget, string, EventListenerOrEventListenerObject, unknown][] = [];
const realAdd = new Map<EventTarget, EventTarget["addEventListener"]>([[window, window.addEventListener], [document, document.addEventListener]]);
beforeEach(() => {
  for (const target of [window, document] as EventTarget[]) {
    const add = realAdd.get(target)!.bind(target);
    target.addEventListener = ((type: string, fn: EventListenerOrEventListenerObject, options?: unknown) => {
      listeners.push([target, type, fn, options]);
      add(type, fn, options as AddEventListenerOptions);
    }) as typeof target.addEventListener;
  }
});
afterEach(() => {
  for (const [target, type, fn, options] of listeners.splice(0)) target.removeEventListener(type, fn, options as AddEventListenerOptions);
  for (const [target, add] of realAdd) target.addEventListener = add;
  expect(dialogs).toEqual([]);
  timing.holdMs = 600;
  if (savedUrl !== undefined) happy().setURL(savedUrl);
  savedUrl = undefined;
});

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
    button("Fill anyway")!.click();
    await until(() => main().textContent!.includes("Not filled"));
    expect(asked.find((m) => m.kind === "fillTab")).toMatchObject({ force: true, tabId: 5 });
  });
});

describe("platform wording", () => {
  test("on Linux the popup names the master password, not Windows Hello", async () => {
    await boot({}, {}, "linux");
    await until(() => !!button("Show all my logins"));
    expect(main().textContent).toContain("your master password");
    expect(main().textContent).not.toContain("Windows Hello");
    button("Show all my logins")!.click();
    await until(() => !!button("Fill"));
    button("Fill")!.click();
    await until(() => !!button("Fill anyway"));
    expect(main().textContent).toContain("your master password");
  });

  test("on Windows the fill confirmation names Windows Hello", async () => {
    await boot();
    await until(() => !!button("Show all my logins"));
    button("Show all my logins")!.click();
    await until(() => !!button("Fill"));
    button("Fill")!.click();
    await until(() => !!button("Fill anyway"));
    expect(main().textContent).toContain("ask for Windows Hello before it fills");
  });

  test("the privacy note names the running browser, not Chrome and Edge", async () => {
    await boot();
    const note = () => document.getElementById("privacy-note")!.textContent ?? "";
    await until(() => note() !== "");
    // happy-dom's user agent has no browser brand: the default name is Chrome.
    expect(note()).toBe("Chrome may still offer to save passwords. Turn this on to use only LocalPass.");
    expect(note()).not.toContain("Edge");
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

// ---- 8.3c: the first Fill has the focus in a popup the user opened; one the background showed is held ----

const OWN = [
  { id: "m1", title: "Mail", username: "ann", url: "https://a.example", exact: true, hasPassword: true, hasTotp: false },
  { id: "m2", title: "Mail 2", username: "ben", url: "https://a.example", exact: true, hasPassword: true, hasTotp: false },
];
const withLogins = { logins: OWN };
const active = () => document.activeElement as HTMLElement | null;
const fills = () => asked.filter((m) => m.kind === "fillTab");
/** A key press on the focused element as a browser does it: the button's own activation runs unless the key down was cancelled. */
function press(key: "Enter" | " ", init: KeyboardEventInit = {}) {
  const target = active() ?? document.body;
  const down = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(down);
  const up = new KeyboardEvent("keyup", { key, bubbles: true, cancelable: true });
  target.dispatchEvent(up);
  const activates = key === "Enter" ? !down.defaultPrevented : !down.defaultPrevented && !up.defaultPrevented;
  if (activates && target instanceof HTMLButtonElement) target.click();
}
const fillButtons = () => [...main().querySelectorAll("ul.list")][0]!.querySelectorAll("button");

describe("the popup the user opened (toolbar icon or shortcut)", () => {
  test("asks on its own port, named with its browser window", async () => {
    await boot(withLogins);
    expect(portName).toBe("popup:20");
    expect(popupPort).toBeDefined();
  });

  test("the first Fill for the tab's site has the focus, and Enter fills that login", async () => {
    await boot(withLogins);
    await until(() => fillButtons().length === 2);
    expect(active()).toBe(fillButtons()[0]!);
    expect(main().querySelector("[aria-disabled]")).toBeNull();
    press("Enter");
    await until(() => fills().length === 1);
    expect(fills()[0]).toMatchObject({ itemId: "m1", force: false, tabId: 5 });
  });

  test("the note may come after the view: the focus then moves to the first Fill", async () => {
    timing.holdMs = 10_000;
    await boot(withLogins, {}, "win", "late");
    await until(() => fillButtons().length === 2);
    expect(active()).not.toBe(fillButtons()[0]!);
    expect(fillButtons()[0]!.getAttribute("aria-disabled")).toBe("true");
    popupPort!.emit({ userOpened: true });
    expect(active()).toBe(fillButtons()[0]!);
    expect(fillButtons()[0]!.hasAttribute("aria-disabled")).toBe(false);
    press("Enter");
    await until(() => fills().length === 1);
  });

  test("a late note does not take the focus from a control the user moved to", async () => {
    timing.holdMs = 10_000;
    await boot(withLogins, {}, "win", "late");
    await until(() => !!button("Show all my logins"));
    button("Show all my logins")!.focus();
    popupPort!.emit({ userOpened: true });
    expect(active()?.textContent).toBe("Show all my logins");
  });

  test("after it lost the focus once, a new view does not focus a Fill again", async () => {
    timing.holdMs = 10_000;
    await boot(withLogins);
    await until(() => active() === fillButtons()[0]);
    window.dispatchEvent(new Event("blur"));
    const old = fillButtons()[0];
    // The background says the connection changed: the popup draws its view again.
    fakeChrome.onMessage!({ kind: "brokerState" }, {}, () => {});
    await until(() => fillButtons()[0] !== old, 1000, "the new view");
    expect(active()).not.toBe(fillButtons()[0]!);
    expect(active()?.tagName).not.toBe("BUTTON");
  });

  test("a key held down (a repeat) never fills", async () => {
    await boot(withLogins);
    await until(() => active() === fillButtons()[0]);
    press("Enter", { repeat: true });
    await sleep(30);
    expect(fills()).toEqual([]);
  });

  test("only logins for the tab's site get the focus: none when there are none, and never a row for another site", async () => {
    await boot({}, { cachedLogins: () => ({ ok: true, data: ITEMS }) });
    await until(() => rows().length === 2);
    expect(active()?.tagName).not.toBe("BUTTON");
    press("Enter");
    await sleep(30);
    expect(fills()).toEqual([]);
  });

  test("after it lost the focus once, coming back holds it again", async () => {
    timing.holdMs = 10_000;
    await boot(withLogins);
    await until(() => active() === fillButtons()[0]);
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
    expect(fillButtons()[0]!.getAttribute("aria-disabled")).toBe("true");
    press("Enter");
    fillButtons()[0]!.click();
    await sleep(30);
    expect(fills()).toEqual([]);
  });
});

describe("a popup the background's own openPopup showed (no note)", () => {
  test("no Fill has the focus, and Enter, Space and clicks do nothing during the hold", async () => {
    timing.holdMs = 10_000;
    await boot(withLogins, {}, "win", "background");
    await until(() => fillButtons().length === 2);
    expect(active()).not.toBe(fillButtons()[0]!);
    expect(active()?.tagName).not.toBe("BUTTON");
    expect(fillButtons()[0]!.getAttribute("aria-disabled")).toBe("true");
    expect(fillButtons()[0]!.disabled).toBe(false); // held, not greyed out
    press("Enter");
    // Even with the focus moved onto a Fill (Tab), nothing acts while held.
    fillButtons()[0]!.focus();
    press("Enter");
    press(" ");
    fillButtons()[0]!.click();
    button("Show all my logins")!.click();
    await sleep(30);
    expect(fills()).toEqual([]);
    expect(count("loadLogins")).toBe(0);
  });

  test("the switches are held too", async () => {
    timing.holdMs = 10_000;
    await boot(withLogins, {}, "win", "background");
    await until(() => fillButtons().length === 2);
    const box = document.getElementById("privacy") as HTMLInputElement;
    await until(() => box.getAttribute("aria-disabled") === "true");
    box.click();
    await sleep(30);
    expect(count("setPrivacy")).toBe(0);
  });

  test("after the hold the user can click, but the focus stays off the Fill", async () => {
    timing.holdMs = 40;
    await boot(withLogins, {}, "win", "background");
    await until(() => fillButtons().length === 2);
    await until(() => !fillButtons()[0]!.hasAttribute("aria-disabled"), 1000, "the hold to end");
    expect(active()).not.toBe(fillButtons()[0]!);
    fillButtons()[1]!.click();
    await until(() => fills().length === 1);
    expect(fills()[0]).toMatchObject({ itemId: "m2" });
  });

  test("no note but exactly {userOpened: true} frees it", async () => {
    timing.holdMs = 10_000;
    for (const note of [{ userOpened: "true" }, { userOpened: 1 }, {}, null, "userOpened", { userOpened: [true] }]) {
      await boot(withLogins, {}, "win", { note });
      await until(() => fillButtons().length === 2);
      await sleep(10);
      expect(fillButtons()[0]!.getAttribute("aria-disabled"), JSON.stringify(note)).toBe("true");
      expect(active()).not.toBe(fillButtons()[0]!);
    }
  });
});

describe("a popup that a decision panel turned into (`from=panel`)", () => {
  // The panel may have used up the background's count of its own openPopup when it attached: the background would then
  // say the user opened this popup (the fake below answers {userOpened: true} on any port). It must not be believed.
  test("never asks, stays held and focuses no Fill, even where the background would say the user opened it", async () => {
    timing.holdMs = 10_000;
    await boot(withLogins, {}, "win", "user", "?from=panel");
    await until(() => fillButtons().length === 2);
    await sleep(20);
    expect(portName).toBe("");
    expect(noted).toBe(false);
    expect(active()).not.toBe(fillButtons()[0]!);
    expect(active()?.tagName).not.toBe("BUTTON");
    expect(fillButtons()[0]!.getAttribute("aria-disabled")).toBe("true");
    press("Enter");
    fillButtons()[0]!.click();
    await sleep(30);
    expect(fills()).toEqual([]);
  });

  test("a note on its port is not believed either", async () => {
    timing.holdMs = 10_000;
    await boot(withLogins, {}, "win", "background", "?from=panel");
    await until(() => fillButtons().length === 2);
    // A port it never opened cannot be noted; should one exist, a note on it still frees nothing.
    popupPort?.emit({ userOpened: true });
    await sleep(10);
    expect(fillButtons()[0]!.getAttribute("aria-disabled")).toBe("true");
    expect(active()).not.toBe(fillButtons()[0]!);
  });

  test("after the hold it works by click, and the focus stays off the Fill", async () => {
    timing.holdMs = 40;
    await boot(withLogins, {}, "win", "user", "?from=panel");
    await until(() => fillButtons().length === 2);
    await until(() => !fillButtons()[0]!.hasAttribute("aria-disabled"), 1000, "the hold to end");
    expect(active()).not.toBe(fillButtons()[0]!);
    fillButtons()[1]!.click();
    await until(() => fills().length === 1);
    expect(fills()[0]).toMatchObject({ itemId: "m2" });
  });

  test("the normal popup (no marker, or another `from`) still asks, and the user's popup focuses its first Fill", async () => {
    for (const search of ["", "?from=toolbar", "?from=panelx"]) {
      await boot(withLogins, {}, "win", "user", search);
      if (search) await until(() => noted, 1000, "the background's note");
      await until(() => active() === fillButtons()[0], 1000, `the first Fill focused (${search || "no query"})`);
      expect(portName).toBe("popup:20");
      expect(main().querySelector("[aria-disabled]")).toBeNull();
    }
  });
});

describe("inline messages instead of alert and confirm", () => {
  test("a login for another site asks in the row first: Back goes back, Fill anyway fills with the app's check", async () => {
    await boot();
    button("Show all my logins")!.click();
    await until(() => rows().length === 2);
    const row = () => main().querySelectorAll("ul.list li")[0] as HTMLElement;
    (row().querySelector("button") as HTMLButtonElement).click();
    await until(() => !!button("Fill anyway"));
    expect(row().textContent).toContain("Saved for a different website than a.example");
    expect(active()?.textContent).toBe("Back"); // a second Enter does not confirm
    expect(fills()).toEqual([]);
    press("Enter");
    await until(() => !button("Fill anyway"));
    expect(active()?.textContent).toBe("Fill");
    expect(fills()).toEqual([]);
    active()!.click();
    await until(() => !!button("Fill anyway"));
    button("Fill anyway")!.click();
    await until(() => fills().length === 1);
    expect(fills()[0]).toMatchObject({ force: true, itemId: "1" });
  });

  test("a failed fill is shown in its row, as text", async () => {
    await boot(withLogins, { fillTab: () => ({ ok: false, code: "error", message: "<b>The page changed</b>" }) });
    await until(() => active() === fillButtons()[0]);
    press("Enter");
    const status = () => main().querySelector("ul.list li .who .error");
    await until(() => !!status());
    expect(status()!.textContent).toBe("<b>The page changed</b>");
    expect(status()!.querySelector("b")).toBeNull();
  });

  test("a failed Unlock is shown under its button", async () => {
    await boot({ vault: "locked" }, { unlock: () => ({ ok: false, code: "error", message: "The app did not answer" }) });
    await until(() => !!button("Unlock"));
    button("Unlock")!.click();
    await until(() => main().querySelector("p.error")?.textContent === "The app did not answer");
    expect(button("Unlock")).toBeDefined();
  });

  test("a failed change of the browser's own password saving is shown in its note", async () => {
    await boot({}, { setPrivacy: () => ({ ok: false, code: "error", message: "Not allowed here" }) });
    const box = document.getElementById("privacy") as HTMLInputElement;
    const note = document.getElementById("privacy-note")!;
    await until(() => note.textContent !== "");
    box.click();
    await until(() => note.textContent === "Not allowed here");
    expect(note.className).toContain("error");
  });
});
