// The decision window's page (src/decide): what it shows for each decision, that page-supplied
// text is only ever text, and which messages its buttons send.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { timing } from "../src/decide/timing";
import { type Json, installChrome, sleep, until } from "./chrome";

const HTML = `<header><span id="site"></span></header><main id="main"></main>`;
let sent: Json[];
/** What the background answers, by message kind (or by kind and then the next). */
let answers: Record<string, (message: Json) => unknown>;
let copies = 0;
let savedUrl = "";

const happy = () => (window as unknown as { happyDOM: { setURL(u: string): void } }).happyDOM;

let disconnectPanel: (() => void) | undefined;
/** What the page listens to on its port (only the background can send there): undefined when it does not listen. */
let notePanel: ((note: unknown) => void) | undefined;
/** The system `runtime.getPlatformInfo` reports to the next page ("win" unless a test changes it). */
let nextOs = "win";
async function openPage(info: Json, extra: Record<string, (message: Json) => unknown> = {}, panel = false) {
  installChrome().os = nextOs;
  sent = [];
  answers = { decisionInfo: () => ({ ok: true, data: info }), ...extra };
  const chromeApi = (globalThis as unknown as { chrome: { runtime: Json } }).chrome;
  disconnectPanel = undefined;
  notePanel = undefined;
  chromeApi.runtime.connect = () => ({
    onDisconnect: { addListener: (fn: () => void) => { disconnectPanel = fn; } },
    onMessage: { addListener: (fn: (note: unknown) => void) => { notePanel = fn; } },
  });
  chromeApi.runtime.sendMessage = async (message: Json) => {
    sent.push(message);
    return answers[message.kind as string]?.(message) ?? { ok: true, data: {} };
  };
  document.body.innerHTML = HTML;
  happy().setURL(`moz-extension://uuid/decide.html?id=abc${panel ? "&presentation=panel" : ""}`);
  await import(`../src/decide/index.ts?page=${copies++}`);
  await sleep(10);
}

const main = () => document.getElementById("main")!;
const buttons = () => [...main().querySelectorAll("button")];
const button = (label: string) => buttons().find((b) => b.textContent === label)!;
/** In the hold: marked for assistive technology, and never `disabled` (which would grey it out). */
const held = (b: HTMLButtonElement) => {
  expect(b.disabled).toBe(false);
  return b.getAttribute("aria-disabled") === "true";
};

/** Every page the tests import adds listeners to the shared window and document; they are removed after each test, so an earlier page cannot answer a later test's key press. */
const listeners: [EventTarget, string, EventListenerOrEventListenerObject, unknown][] = [];
const realHasFocus = document.hasFocus;
const realAdd = new Map<EventTarget, EventTarget["addEventListener"]>([[window, window.addEventListener], [document, document.addEventListener]]);

beforeEach(() => {
  savedUrl = window.location.href;
  // Tests act at once and do not wait for pings (the tests of the rules below turn them on).
  timing.holdMs = 0;
  timing.keepAliveMs = 3_600_000;
  timing.outsideMs = 20;
  timing.pollMs = 500;
  for (const target of [window, document] as EventTarget[]) {
    const add = realAdd.get(target)!.bind(target);
    target.addEventListener = ((type: string, fn: EventListenerOrEventListenerObject, options?: unknown) => {
      listeners.push([target, type, fn, options]);
      add(type, fn, options as AddEventListenerOptions);
    }) as typeof target.addEventListener;
  }
});
/** Removes what the pages imported so far listen with, so only the next page reacts to keys and focus. */
function dropListeners() {
  for (const [target, type, fn, options] of listeners.splice(0)) target.removeEventListener(type, fn, options as AddEventListenerOptions);
}
afterEach(() => {
  dropListeners();
  for (const [target, add] of realAdd) target.addEventListener = add;
  document.hasFocus = realHasFocus;
  happy().setURL(savedUrl);
  document.body.innerHTML = "";
});

const CREATE_INFO = {
  decision: "passkeyCreate",
  origin: "https://login.example.com",
  host: "login.example.com",
  request: { rpId: "example.com", rpName: "<img src=x onerror=alert(1)>", userName: "alice", userDisplayName: "Alice", userHandle: "AA", challenge: "AAAA", algorithms: [-7], exclude: [], uv: "preferred" },
};

describe("toolbar panel lifecycle", () => {
  test("uses a bounded panel layout and closes when its background port ends", async () => {
    const saved = window.close;
    let closed = 0;
    window.close = () => { closed++; };
    try {
      await openPage({ decision: "chooseLogin", host: "example.com", origin: "https://example.com" }, {
        decisionLogins: () => ({ ok: true, data: { status: "ok", logins: [] } }),
      }, true);
      expect(document.body.classList.contains("panel")).toBe(true);
      expect(main().textContent).toContain("No saved logins");
      disconnectPanel!();
      expect(closed).toBe(1);
    } finally {
      window.close = saved;
    }
  });

  test("a successful choice closes the panel without exposing any credential in the page", async () => {
    const saved = window.close;
    let closed = 0;
    window.close = () => { closed++; };
    try {
      await openPage({ decision: "chooseLogin", host: "example.com", origin: "https://example.com" }, {
        decisionLogins: () => ({ ok: true, data: { status: "ok", logins: [{ id: "i1", title: "Example", username: "alice" }] } }),
      }, true);
      await until(() => !!main().querySelector(".row"));
      (main().querySelector(".row") as HTMLButtonElement).click();
      await until(() => closed === 1);
      expect(sent.at(-1)).toEqual({ kind: "decisionAnswer", id: "abc", answer: { action: "fill", itemId: "i1" } });
    } finally {
      window.close = saved;
    }
  });
});

describe("create a passkey", () => {
  test("names the verified site first, shows page text only as text, and asks for the decision by its id", async () => {
    await openPage(CREATE_INFO);
    expect(sent[0]).toEqual({ kind: "decisionInfo", id: "abc" });
    expect(document.getElementById("site")!.textContent).toBe("login.example.com");
    expect(main().querySelector(".lead")!.textContent).toBe("example.com");
    expect(main().querySelector("img")).toBeNull();
    expect(main().textContent).toContain("<img src=x onerror=alert(1)>");
    expect(buttons().map((b) => b.textContent)).toEqual(["Create passkey", "Use another method", "Cancel"]);
  });

  test("Create sends the answer only; Use another method and Cancel send theirs", async () => {
    await openPage(CREATE_INFO);
    button("Create passkey").click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toEqual({ kind: "decisionAnswer", id: "abc", answer: { action: "create" } });
    await sleep(10);
    await openPage(CREATE_INFO);
    button("Use another method").click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toMatchObject({ answer: { action: "fallback" } });
  });

  test("a refusal is shown in the window, which stays for the user to close or hand over", async () => {
    await openPage(CREATE_INFO);
    answers.decisionAnswer = () => ({ ok: false, code: "denied", message: "You said no." });
    button("Create passkey").click();
    await until(() => main().textContent!.includes("Nothing was changed."));
    expect(main().textContent).toContain("You said no.");
    expect(buttons().map((b) => b.textContent)).toEqual(["Use another method", "Close"]);
  });
});

describe("sign in with a passkey", () => {
  const GET_INFO = { decision: "passkeyGet", origin: "https://login.example.com", host: "login.example.com", rpId: "example.com" };
  const found = [{ itemId: "k1", rpId: "example.com", rpName: "Example", userName: "alice", userDisplayName: "Alice", credentialId: "c", userHandle: "u", createdMs: 0 }];

  test("none found, or the app is unavailable: the request goes back to the browser at once", async () => {
    for (const reply of [{ ok: true, data: [] }, { ok: false, code: "appNotRunning", message: "x" }]) {
      installChrome();
      sent = [];
      const chromeApi = (globalThis as unknown as { chrome: { runtime: Json } }).chrome.runtime;
      chromeApi.connect = () => ({});
      chromeApi.sendMessage = async (m: Json) => (sent.push(m), m.kind === "decisionInfo" ? { ok: true, data: GET_INFO } : m.kind === "decisionCandidates" ? reply : { ok: true, data: {} });
      document.body.innerHTML = HTML;
      happy().setURL("moz-extension://uuid/decide.html?id=abc");
      await import(`../src/decide/index.ts?page=${copies++}`);
      await until(() => sent.some((m) => m.kind === "decisionAnswer"));
      expect(sent.at(-1)).toMatchObject({ answer: { action: "fallback" } });
    }
  });

  test("locked: offers Unlock, then looks again and signs the chosen passkey", async () => {
    installChrome();
    sent = [];
    let locked = true;
    const chromeApi = (globalThis as unknown as { chrome: { runtime: Json } }).chrome.runtime;
    chromeApi.connect = () => ({});
    chromeApi.sendMessage = async (m: Json) => {
      sent.push(m);
      if (m.kind === "decisionInfo") return { ok: true, data: GET_INFO };
      // The panel's Unlock goes to the background with the decision's id (it follows the Unlock after the panel closed).
      if (m.kind === "decisionUnlock") return m.id === "abc" ? ((locked = false), { ok: true, data: true }) : { ok: false, code: "notFound", message: "x" };
      if (m.kind === "decisionCandidates") return locked ? { ok: false, code: "locked", message: "locked" } : { ok: true, data: found };
      return { ok: true, data: {} };
    };
    document.body.innerHTML = HTML;
    happy().setURL("moz-extension://uuid/decide.html?id=abc");
    await import(`../src/decide/index.ts?page=${copies++}`);
    await until(() => !!button("Unlock"));
    button("Unlock").click();
    await until(() => buttons().some((b) => b.className === "row"));
    expect(main().querySelector(".row")!.textContent).toContain("alice");
    main().querySelector<HTMLButtonElement>(".row")!.click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toMatchObject({ answer: { action: "assert", itemId: "k1" } });
  });
});

describe("save a login", () => {
  test("shows who and where, and sends Save, Not now or Never", async () => {
    const info = { decision: "saveLogin", origin: "https://a.example", host: "a.example", username: "alice" };
    await openPage(info);
    expect(main().textContent).toContain("alice on a.example");
    answers.decisionAnswer = () => ({ ok: true, data: { message: "Saved to LocalPass." } });
    button("Save").click();
    await until(() => main().textContent!.includes("Saved to LocalPass."));
    for (const [label, action] of [["Not now", "cancel"], ["Never for this site", "never"]] as const) {
      await openPage(info);
      button(label).click();
      await until(() => sent.some((m) => m.kind === "decisionAnswer"));
      expect(sent.at(-1)).toMatchObject({ answer: { action } });
    }
  });
});

describe("save or update (8.2)", () => {
  test("a new login is offered as a save", async () => {
    await openPage({ decision: "saveLogin", origin: "https://a.example", host: "a.example", username: "alice", update: false });
    expect(main().querySelector(".lead")!.textContent).toBe("Save this login to LocalPass?");
    expect(button("Save")).toBeDefined();
    expect(button("Update")).toBeUndefined();
  });

  test("a changed password is offered as an update, and Update sends the same save answer", async () => {
    await openPage({ decision: "saveLogin", origin: "https://a.example", host: "a.example", username: "alice", update: true });
    expect(main().querySelector(".lead")!.textContent).toBe("Update the saved password in LocalPass?");
    expect(main().textContent).toContain("alice on a.example");
    expect(button("Save")).toBeUndefined();
    button("Update").click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toMatchObject({ answer: { action: "save" } });
  });
});

describe("choose a login", () => {
  const info = { decision: "chooseLogin", origin: "https://a.example", host: "a.example" };

  test("lists the logins for the site and fills the chosen one", async () => {
    installChrome();
    sent = [];
    const chromeApi = (globalThis as unknown as { chrome: { runtime: Json } }).chrome.runtime;
    chromeApi.connect = () => ({});
    chromeApi.sendMessage = async (m: Json) => {
      sent.push(m);
      if (m.kind === "decisionInfo") return { ok: true, data: info };
      if (m.kind === "decisionLogins") return { ok: true, data: { status: "ok", logins: [{ id: "i1", title: "Example", username: "alice" }] } };
      return { ok: true, data: {} };
    };
    document.body.innerHTML = HTML;
    happy().setURL("moz-extension://uuid/decide.html?id=abc");
    await import(`../src/decide/index.ts?page=${copies++}`);
    await until(() => !!main().querySelector(".row"));
    main().querySelector<HTMLButtonElement>(".row")!.click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toEqual({ kind: "decisionAnswer", id: "abc", answer: { action: "fill", itemId: "i1" } });
  });

  test("says why there is nothing to choose from", async () => {
    installChrome();
    const chromeApi = (globalThis as unknown as { chrome: { runtime: Json } }).chrome.runtime;
    chromeApi.connect = () => ({});
    chromeApi.sendMessage = async (m: Json) => (m.kind === "decisionInfo" ? { ok: true, data: info } : { ok: true, data: { status: "noVault", logins: [] } });
    document.body.innerHTML = HTML;
    happy().setURL("moz-extension://uuid/decide.html?id=abc");
    await import(`../src/decide/index.ts?page=${copies++}`);
    await until(() => main().textContent!.includes("Create or join a vault"));
  });
});

describe("a request that is gone", () => {
  test("says so", async () => {
    await openPage(CREATE_INFO);
    installChrome();
    const chromeApi = (globalThis as unknown as { chrome: { runtime: Json } }).chrome.runtime;
    chromeApi.connect = () => ({});
    chromeApi.sendMessage = async () => ({ ok: false, code: "notFound", message: "this request is no longer open" });
    document.body.innerHTML = HTML;
    await import(`../src/decide/index.ts?page=${copies++}`);
    await until(() => main().textContent!.includes("no longer open"));
  });
});

describe("the window cannot be clicked into an action (a page chooses when it opens)", () => {
  test("action buttons and rows are held (not greyed) for the hold, safe buttons are not, and nothing that acts has focus", async () => {
    timing.holdMs = 120;
    await openPage(CREATE_INFO);
    expect(held(button("Create passkey"))).toBe(true);
    expect(held(button("Use another method"))).toBe(false);
    expect(held(button("Cancel"))).toBe(false);
    expect(document.activeElement?.textContent).not.toBe("Create passkey");
    button("Create passkey").click(); // a key or click that lands early
    await sleep(30);
    expect(sent.some((m) => m.kind === "decisionAnswer")).toBe(false);
    await until(() => !held(button("Create passkey")));
    button("Create passkey").click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toMatchObject({ answer: { action: "create" } });
  });

  test("coming back to the window starts the wait again", async () => {
    timing.holdMs = 60;
    await openPage(CREATE_INFO);
    await until(() => !held(button("Create passkey")));
    window.dispatchEvent(new Event("focus"));
    expect(held(button("Create passkey"))).toBe(true);
    await until(() => !held(button("Create passkey")));
  });

  test("login rows are held too", async () => {
    timing.holdMs = 80;
    installChrome();
    sent = [];
    const chromeApi = (globalThis as unknown as { chrome: { runtime: Json } }).chrome.runtime;
    chromeApi.connect = () => ({});
    chromeApi.sendMessage = async (m: Json) =>
      m.kind === "decisionInfo"
        ? { ok: true, data: { decision: "chooseLogin", origin: "https://a.example", host: "a.example" } }
        : m.kind === "decisionLogins"
          ? { ok: true, data: { status: "ok", logins: [{ id: "i1", title: "Example", username: "alice" }] } }
          : { ok: true, data: {} };
    document.body.innerHTML = HTML;
    happy().setURL("moz-extension://uuid/decide.html?id=abc");
    await import(`../src/decide/index.ts?page=${copies++}`);
    await until(() => !!main().querySelector(".row"));
    expect(held(main().querySelector<HTMLButtonElement>(".row")!)).toBe(true);
  });
});

describe("keeping the background awake", () => {
  test("pings while the user decides, and stops once an answer worked (the save result must stay on screen)", async () => {
    timing.keepAliveMs = 25;
    await openPage({ decision: "saveLogin", origin: "https://a.example", host: "a.example", username: "alice" });
    const pings = () => sent.filter((m) => m.kind === "decisionInfo").length;
    await until(() => pings() >= 3, 1000, "pings");
    // The background forgets the decision as soon as the save went through, while the window lingers.
    answers.decisionAnswer = () => ({ ok: true, data: { message: "Saved to LocalPass." } });
    answers.decisionInfo = () => ({ ok: false, code: "notFound", message: "this request is no longer open" });
    button("Save").click();
    await until(() => main().textContent!.includes("Saved to LocalPass."));
    await sleep(150);
    expect(main().textContent).toBe("Saved to LocalPass.");
  });

  test("a failed answer resumes the pings, and a forgotten decision is reported", async () => {
    timing.keepAliveMs = 25;
    await openPage(CREATE_INFO);
    answers.decisionAnswer = () => ({ ok: false, code: "denied", message: "no" });
    button("Create passkey").click();
    await until(() => main().textContent!.includes("Nothing was changed."));
    const before = sent.filter((m) => m.kind === "decisionInfo").length;
    await until(() => sent.filter((m) => m.kind === "decisionInfo").length > before, 1000, "a ping after the failure");
    answers.decisionInfo = () => ({ ok: false, code: "notFound", message: "gone" });
    await until(() => main().textContent!.includes("LocalPass forgot it"));
  });
});

describe("closing without choosing: Esc, and a click outside for choosing and saving", () => {
  const CHOOSE = { decision: "chooseLogin", origin: "https://a.example", host: "a.example" };
  const SAVE = { decision: "saveLogin", origin: "https://a.example", host: "a.example", username: "alice" };
  const GET = { decision: "passkeyGet", origin: "https://login.example.com", host: "login.example.com", rpId: "example.com" };
  const logins = () => ({ ok: true, data: { status: "ok", logins: [{ id: "i1", title: "Example", username: "alice" }] } });
  const never = () => new Promise(() => {});
  let focused = true;
  const sentAnswers = () => sent.filter((m) => m.kind === "decisionAnswer");

  /** The window starts focused (or not), and `focused` is what `document.hasFocus()` says from then on. */
  async function open(info: Json, extra: Record<string, (message: Json) => unknown> = {}, startFocused = true, panel = false) {
    dropListeners(); // a test that opens several windows in turn: the earlier ones are gone
    focused = startFocused;
    document.hasFocus = () => focused;
    await openPage(info, extra, panel);
  }
  const press = (init: KeyboardEventInit = {}) => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, ...init }));
  const lose = () => {
    focused = false;
    window.dispatchEvent(new Event("blur"));
  };
  const gain = () => {
    focused = true;
    window.dispatchEvent(new Event("focus"));
  };
  const cancelled = () => expect(sentAnswers().at(-1)).toEqual({ kind: "decisionAnswer", id: "abc", answer: { action: "cancel" } });

  test("Esc does what Cancel does, even while the window still holds its acting controls", async () => {
    timing.holdMs = 150;
    await open(CREATE_INFO);
    expect(held(button("Create passkey"))).toBe(true);
    press();
    await until(() => sentAnswers().length === 1);
    cancelled();
  });

  test("Esc is the way out of every view: the chooser, the save offer (Not now) and an error (Close)", async () => {
    await open(CHOOSE, { decisionLogins: logins });
    await until(() => !!main().querySelector(".row"));
    press();
    await until(() => sentAnswers().length === 1);
    cancelled();
    await open(SAVE);
    press();
    await until(() => sentAnswers().length === 1);
    cancelled();
    await open(CHOOSE, { decisionLogins: () => ({ ok: true, data: { status: "locked", logins: [] } }) });
    await until(() => !!button("Unlock"));
    press();
    await until(() => sentAnswers().length === 1);
    cancelled();
    await open(CHOOSE, { decisionLogins: () => ({ ok: true, data: { status: "appNotRunning", logins: [] } }) });
    await until(() => main().textContent!.includes("Can't reach LocalPass"));
    press();
    await until(() => sentAnswers().length === 1);
    cancelled();
  });

  test("a held key, a composition, a modifier or another key is not Esc; one Esc sends one answer", async () => {
    await open(SAVE);
    press({ repeat: true });
    press({ isComposing: true });
    press({ ctrlKey: true });
    press({ shiftKey: true });
    press({ key: "Enter" });
    await sleep(60);
    expect(sentAnswers()).toEqual([]);
    press();
    press();
    await sleep(60);
    expect(sentAnswers().length).toBe(1);
  });

  test("the hint is shown where Esc works, and not on a view that only reports", async () => {
    await open(CREATE_INFO);
    expect(main().querySelector(".hint")!.textContent).toBe("Esc to close");
    await open(SAVE, { decisionAnswer: () => ({ ok: true, data: { message: "Saved to LocalPass." } }) });
    button("Save").click();
    await until(() => main().textContent!.includes("Saved to LocalPass."));
    expect(main().querySelector(".hint")).toBeNull();
    press();
    await sleep(60);
    expect(sentAnswers().length).toBe(1); // only the Save answer
  });

  test("Esc in the waiting view is a Cancel, as the button is", async () => {
    await open(CHOOSE, { decisionLogins: logins, decisionAnswer: never });
    await until(() => !!main().querySelector(".row"));
    main().querySelector<HTMLButtonElement>(".row")!.click();
    await until(() => main().textContent!.includes("Waiting for LocalPass"));
    press();
    await until(() => sentAnswers().length === 2);
    cancelled();
  });

  test("a chooser or a save offer that lost focus and did not get it back is closed", async () => {
    await open(CHOOSE, { decisionLogins: logins });
    await until(() => !!main().querySelector(".row"));
    lose();
    await until(() => sentAnswers().length === 1);
    cancelled();
    await open(SAVE);
    lose();
    await until(() => sentAnswers().length === 1);
    cancelled();
  });

  test("focus that comes straight back keeps the window", async () => {
    await open(CHOOSE, { decisionLogins: logins });
    await until(() => !!main().querySelector(".row"));
    lose();
    gain();
    await sleep(80);
    window.dispatchEvent(new Event("blur")); // a blur while the window still has focus (a tooltip, the title bar)
    await sleep(80);
    expect(sentAnswers()).toEqual([]);
    lose(); // and a real loss afterwards still closes it
    await until(() => sentAnswers().length === 1);
  });

  test("a window that never had focus is not closed by a blur, and is once it has had it", async () => {
    await open(SAVE, {}, false);
    window.dispatchEvent(new Event("blur"));
    await sleep(80);
    expect(sentAnswers()).toEqual([]);
    gain();
    lose();
    await until(() => sentAnswers().length === 1);
    cancelled();
  });

  test("losing focus while an answer is carried out does nothing; after a refusal the window is armed again", async () => {
    let refuse: (reply: unknown) => void = () => {};
    await open(CHOOSE, { decisionLogins: logins, decisionAnswer: () => new Promise((resolve) => (refuse = resolve)) });
    await until(() => !!main().querySelector(".row"));
    main().querySelector<HTMLButtonElement>(".row")!.click();
    await until(() => sentAnswers().length === 1);
    lose(); // the LocalPass app raises its own confirm dialog now
    await sleep(80);
    expect(sentAnswers().length).toBe(1);
    refuse({ ok: false, code: "denied", message: "no" });
    await until(() => main().textContent!.includes("Nothing was changed."));
    gain();
    lose();
    await until(() => sentAnswers().length === 2);
    cancelled();
  });

  test("losing focus while the app's Unlock is on screen does nothing, and the chooser that follows is armed", async () => {
    let unlocked = false;
    let finish: (reply: unknown) => void = () => {};
    await open(CHOOSE, {
      decisionLogins: () => (unlocked ? logins() : { ok: true, data: { status: "locked", logins: [] } }),
      decisionUnlock: () => new Promise((resolve) => (finish = resolve)),
    });
    await until(() => !!button("Unlock"));
    button("Unlock").click();
    lose(); // the app's own prompt took focus
    await sleep(80);
    expect(sentAnswers()).toEqual([]);
    unlocked = true;
    finish({ ok: true, data: {} });
    await until(() => !!main().querySelector(".row"));
    gain();
    lose();
    await until(() => sentAnswers().length === 1);
    cancelled();
  });

  test("a passkey window is closed by Esc only: a stray click on the page must not fail the site's WebAuthn call", async () => {
    await open(CREATE_INFO);
    lose();
    await sleep(80);
    expect(sentAnswers()).toEqual([]);
    await open(GET, { decisionCandidates: () => ({ ok: true, data: [{ itemId: "k1", rpId: "example.com", rpName: "Example", userName: "alice", userDisplayName: "Alice", credentialId: "c", userHandle: "u", createdMs: 0 }] }) });
    await until(() => !!main().querySelector(".row"));
    lose();
    await sleep(80);
    expect(sentAnswers()).toEqual([]);
    press();
    await until(() => sentAnswers().length === 1);
    cancelled();
  });

  test("before the decision is known nothing closes the window", async () => {
    await open({ decision: "saveLogin" }, { decisionInfo: never });
    lose();
    await sleep(80);
    press();
    await sleep(40);
    expect(sentAnswers()).toEqual([]);
  });

  test("in the toolbar panel a click outside does not answer a save offer (it waits); a login pick still closes", async () => {
    await open(SAVE, {}, true, true);
    lose();
    await sleep(80);
    expect(sentAnswers()).toEqual([]);
    // Esc still is "Not now" there.
    press();
    await until(() => sentAnswers().length === 1);
    cancelled();
    await open(CHOOSE, { decisionLogins: logins }, true, true);
    await until(() => !!main().querySelector(".row"));
    lose();
    await until(() => sentAnswers().length === 1);
    cancelled();
  });
});

describe("a panel that is shown again (8.3a)", () => {
  const SAVE = { decision: "saveLogin", origin: "https://a.example", host: "a.example", username: "alice" };

  test("a panel whose request the background no longer has turns into the normal popup", async () => {
    const replaced: string[] = [];
    const real = window.location.replace;
    window.location.replace = ((url: string) => void replaced.push(url)) as typeof window.location.replace;
    try {
      await openPage({}, { decisionInfo: () => ({ ok: false, code: "notFound", message: "this request is no longer open" }) }, true);
      await until(() => replaced.length === 1);
      // Marked, so that popup stays held and focuses no Fill (this panel may have used up the background's count).
      expect(replaced[0]).toBe("chrome-extension://khlgdnhadbjfghhonaohnhdmedgnenlc/popup.html?from=panel");
      expect(main().textContent).not.toContain("no longer open");
    } finally {
      window.location.replace = real;
    }
  });

  test("a panel whose request ends while it is shown also turns into the marked popup (keep-alive)", async () => {
    const replaced: string[] = [];
    const real = window.location.replace;
    window.location.replace = ((url: string) => void replaced.push(url)) as typeof window.location.replace;
    timing.keepAliveMs = 20;
    let gone = false;
    try {
      await openPage(SAVE, {
        decisionInfo: () => (gone ? { ok: false, code: "notFound", message: "this request is no longer open" } : { ok: true, data: SAVE }),
      }, true);
      await until(() => main().textContent!.includes("Save this login"));
      gone = true; // e.g. the page withdrew its request after this panel attached
      await until(() => replaced.length === 1);
      expect(replaced[0]).toBe("chrome-extension://khlgdnhadbjfghhonaohnhdmedgnenlc/popup.html?from=panel");
    } finally {
      window.location.replace = real;
    }
  });

  test("shown again while LocalPass still works on an earlier answer: it waits, then shows the offer with what went wrong", async () => {
    timing.pollMs = 10;
    let working = true;
    await openPage(SAVE, {
      decisionInfo: () => ({ ok: true, data: working ? { ...SAVE, working: true } : { ...SAVE, failed: "<b>the app went away</b>" } }),
    }, true);
    expect(main().textContent).toContain("Waiting for LocalPass");
    expect(buttons().map((b) => b.textContent)).toEqual(["Cancel"]);
    working = false;
    await until(() => main().textContent!.includes("Save this login"));
    expect(main().textContent).toContain("The last try did not work: <b>the app went away</b>");
    expect(main().querySelector("b")).toBeNull();
    expect(buttons().map((b) => b.textContent)).toEqual(["Save", "Not now", "Never for this site"]);
  });

  test("a waiting passkey request shown again offers the request again, with the last failure as text", async () => {
    await openPage({ ...CREATE_INFO, failed: "Could not reach the app" }, {}, true);
    expect(main().textContent).toContain("The last try did not work: Could not reach the app");
    expect(buttons().map((b) => b.textContent)).toEqual(["Create passkey", "Use another method", "Cancel"]);
  });
});

describe("the picker's own rows (8.3b)", () => {
  const info = (extra: Json = {}) => ({ decision: "chooseLogin", origin: "https://a.example", host: "a.example", newPassword: false, passkeys: false, ...extra });
  const logins = (list: Json[] = [{ id: "i1", title: "Example", username: "alice" }]) => () => ({ ok: true, data: { status: "ok", logins: list } });
  const found = [{ itemId: "k1", rpId: "a.example", rpName: "<b>A</b>", userName: "alice", userDisplayName: "Alice", credentialId: "c", userHandle: "u", createdMs: 0 }];
  const rows = () => [...main().querySelectorAll(".row")].map((r) => r.textContent);

  test("a new-password form gets Generate after the logins, and it sends only that answer", async () => {
    await openPage(info({ newPassword: true }), { decisionLogins: logins() });
    await until(() => rows().length === 2);
    expect(rows()).toEqual(["Examplealice", "Generate a strong passwordFills the new password boxes"]);
    [...main().querySelectorAll<HTMLButtonElement>(".row")][1]!.click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toEqual({ kind: "decisionAnswer", id: "abc", answer: { action: "generate" } });
  });

  test("with no saved login, Generate is still offered; without the hint it is not", async () => {
    await openPage(info({ newPassword: true }), { decisionLogins: logins([]) });
    await until(() => rows().length === 1);
    expect(main().querySelector(".lead")!.textContent).toBe("No saved logins for a.example yet.");
    expect(rows()).toEqual(["Generate a strong passwordFills the new password boxes"]);
    await openPage(info(), { decisionLogins: logins() });
    await until(() => rows().length === 1);
    expect(rows().join()).not.toContain("Generate");
    expect(sent.some((m) => m.kind === "decisionCandidates")).toBe(false);
  });

  test("a page's pending passkey sign-in: its passkeys come first, page text only as text, and a pick signs", async () => {
    await openPage(info({ passkeys: true }), { decisionLogins: logins(), decisionCandidates: () => ({ ok: true, data: found }) });
    await until(() => rows().length === 2);
    expect(rows()[0]).toContain("Passkey: alice");
    expect(rows()[0]).toContain("a.example (<b>A</b>)");
    expect(main().querySelector(".row b")).toBeNull();
    main().querySelector<HTMLButtonElement>(".row")!.click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toEqual({ kind: "decisionAnswer", id: "abc", answer: { action: "assert", itemId: "k1" } });
  });

  test("the new rows are held like every row that acts, and none has the focus", async () => {
    timing.holdMs = 60;
    await openPage(info({ newPassword: true, passkeys: true }), { decisionLogins: logins([]), decisionCandidates: () => ({ ok: true, data: found }) });
    await until(() => rows().length === 2);
    const all = [...main().querySelectorAll<HTMLButtonElement>(".row")];
    expect(all.every((r) => held(r))).toBe(true);
    expect(all.includes(document.activeElement as HTMLButtonElement)).toBe(false);
    await sleep(80);
    expect(all.every((r) => !held(r))).toBe(true);
  });
});

describe("wording for every browser and system (8.3b)", () => {
  afterEach(() => {
    nextOs = "win";
  });

  test("while LocalPass works it names the check of this system: Windows Hello on Windows, the master password on Linux", async () => {
    for (const [os, line] of [["win", "Confirm with Windows Hello if it asks."], ["linux", "Confirm with your master password if it asks."]]) {
      nextOs = os!;
      await openPage(CREATE_INFO, { decisionAnswer: () => new Promise(() => {}) });
      await sleep(10);
      button("Create passkey").click();
      await until(() => main().textContent!.includes("Waiting for LocalPass"));
      expect(main().textContent).toContain(line!);
      if (os === "linux") expect(main().textContent).not.toContain("Windows Hello");
      dropListeners();
    }
  });

  test("no text names a browser", async () => {
    const source = await Bun.file(`${import.meta.dir}/../src/decide/index.ts`).text();
    for (const name of ["Chrome", "Firefox", "Edge", "Chromium"]) expect(source.replace(/\/\/.*$|\/\*[\s\S]*?\*\//gm, "")).not.toContain(name);
  });
});

describe("what the create panel shows (V13, moved from the in-page dialog)", () => {
  const withRequest = (request: Json) => ({ ...CREATE_INFO, request: { ...(CREATE_INFO.request as Json), ...request } });

  test("the first line is the verified relying party id; the page's name and the account follow", async () => {
    await openPage(withRequest({ rpName: "PayPal Secure Login", userName: "alice", userDisplayName: "Alice A." }));
    expect(main().querySelector(".lead")!.textContent).toBe("example.com");
    expect(main().textContent).toContain('The site calls itself "PayPal Secure Login"');
    expect(main().textContent).toContain("Account: alice (Alice A.)");
  });

  test("with no rp id the page's own host is shown", async () => {
    await openPage(withRequest({ rpId: "", rpName: "" }));
    expect(main().querySelector(".lead")!.textContent).toBe("login.example.com");
  });

  test("long names are shortened and control or direction characters are dropped", async () => {
    await openPage(withRequest({ userName: "a".repeat(256), rpName: "\u202eevil\u200b name\u0007\n" }));
    const text = main().textContent!;
    expect(text).not.toContain("a".repeat(81));
    expect(text).toContain("…");
    expect(text).not.toMatch(/[\u202e\u200b\u0007]/);
    expect(text).toContain('"evil name"');
  });
});

describe("keys and the hold without dimming (8.3c)", () => {
  const CHOOSE = { decision: "chooseLogin", origin: "https://a.example", host: "a.example", newPassword: true, passkeys: false };
  const SAVE = { decision: "saveLogin", origin: "https://a.example", host: "a.example", username: "alice", update: false };
  const twoLogins = () => ({ ok: true, data: { status: "ok", logins: [{ id: "i1", title: "One", username: "alice" }, { id: "i2", title: "Two", username: "bob" }] } });
  const rows = () => [...main().querySelectorAll<HTMLButtonElement>(".row")];
  const answers = () => sent.filter((m) => m.kind === "decisionAnswer");
  /** A key on the focused element (or `target`); returns whether the page stopped the browser's own handling. */
  const key = (k: string, init: KeyboardEventInit = {}, type = "keydown", target: EventTarget = document.activeElement ?? document) => {
    const event = new KeyboardEvent(type, { key: k, bubbles: true, cancelable: true, ...init });
    target.dispatchEvent(event);
    return event.defaultPrevented;
  };
  async function picker(panel = true, note?: unknown) {
    dropListeners();
    await openPage(CHOOSE, { decisionLogins: twoLogins, decisionAnswer: () => new Promise(() => {}) }, panel);
    if (note !== undefined) notePanel?.(note);
    await until(() => rows().length === 3);
  }

  test("Down and Up move between the rows (from the safe button: Down to the first, Up to the last), and stop at the ends", async () => {
    await picker();
    expect(document.activeElement?.textContent).toBe("Cancel");
    expect(key("ArrowDown")).toBe(true);
    expect(document.activeElement).toBe(rows()[0]!);
    key("ArrowDown");
    key("ArrowDown");
    key("ArrowDown");
    expect(document.activeElement).toBe(rows()[2]!);
    key("ArrowUp");
    expect(document.activeElement).toBe(rows()[1]!);
    key("ArrowUp");
    key("ArrowUp");
    expect(document.activeElement).toBe(rows()[0]!);
    button("Cancel").focus();
    key("ArrowUp");
    expect(document.activeElement).toBe(rows()[2]!);
    key("ArrowDown", { ctrlKey: true });
    expect(document.activeElement).toBe(rows()[2]!);
    expect(answers()).toEqual([]);
  });

  test("Enter on the focused row picks it; Space does the same", async () => {
    await picker();
    key("ArrowDown");
    key("ArrowDown");
    expect(key("Enter")).toBe(true);
    await until(() => answers().length === 1);
    expect(answers()[0]).toEqual({ kind: "decisionAnswer", id: "abc", answer: { action: "fill", itemId: "i2" } });
    await picker();
    key("ArrowUp"); // the last row: Generate
    expect(key(" ", {}, "keyup")).toBe(true); // the browser's own Space activation (on key up) is stopped
    expect(answers()).toEqual([]);
    expect(key(" ")).toBe(true);
    await until(() => answers().length === 1);
    expect(answers()[0]).toMatchObject({ answer: { action: "generate" } });
  });

  test("during the hold a row ignores a click, Enter and Space (focus can move there, the key still waits); afterwards Enter picks", async () => {
    timing.holdMs = 150;
    await picker();
    key("ArrowDown");
    const row = rows()[0]!;
    expect(document.activeElement).toBe(row);
    expect(held(row)).toBe(true);
    row.click();
    expect(key("Enter")).toBe(true); // the browser's own activation is stopped too
    expect(key(" ")).toBe(true);
    await sleep(30);
    expect(answers()).toEqual([]);
    await until(() => !held(row));
    key("Enter");
    await until(() => answers().length === 1);
    expect(answers()[0]).toMatchObject({ answer: { action: "fill", itemId: "i1" } });
  });

  test("an acting button ignores Enter and Space during the hold too; a safe one is not held", async () => {
    timing.holdMs = 150;
    dropListeners();
    await openPage(CREATE_INFO, { decisionAnswer: () => new Promise(() => {}) }, true);
    const create = button("Create passkey");
    expect(held(create)).toBe(true);
    expect(held(button("Cancel"))).toBe(false);
    expect(button("Cancel").hasAttribute("aria-disabled")).toBe(false);
    create.focus();
    key("Enter");
    key(" ");
    await sleep(30);
    expect(answers()).toEqual([]);
    await until(() => !held(create));
    expect(create.hasAttribute("aria-disabled")).toBe(false);
    key("Enter");
    await until(() => answers().length === 1);
    expect(answers()[0]).toMatchObject({ answer: { action: "create" } });
  });

  test("a key held down never acts, nor one with a modifier or during a composition", async () => {
    await picker();
    key("ArrowDown");
    expect(key("Enter", { repeat: true })).toBe(true);
    expect(key(" ", { repeat: true })).toBe(true);
    key("Enter", { ctrlKey: true });
    key("Enter", { shiftKey: true });
    key("Enter", { isComposing: true });
    await sleep(30);
    expect(answers()).toEqual([]);
    key("Enter");
    await until(() => answers().length === 1);
  });

  test("a panel the user opened acts at once: the background's note ends the hold, and its own first focus does not start it", async () => {
    timing.holdMs = 10_000;
    await picker(true, { userOpened: true });
    expect(rows().every((r) => !held(r))).toBe(true);
    window.dispatchEvent(new Event("focus")); // every panel gets one as it opens (measured)
    document.dispatchEvent(new Event("visibilitychange"));
    expect(rows().every((r) => !held(r))).toBe(true);
    expect(document.activeElement?.textContent).toBe("Cancel"); // nothing that acts has the focus, here too
    key("ArrowDown");
    key("Enter");
    await until(() => answers().length === 1);
    expect(answers()[0]).toMatchObject({ answer: { action: "fill", itemId: "i1" } });
  });

  test("the note may come before or after the first view; a click right after it acts", async () => {
    timing.holdMs = 10_000;
    dropListeners();
    await openPage(SAVE, { decisionAnswer: () => new Promise(() => {}) }, true);
    expect(held(button("Save"))).toBe(true);
    notePanel!({ userOpened: true });
    expect(held(button("Save"))).toBe(false);
    button("Save").click();
    await until(() => answers().length === 1);
  });

  test("a panel the user opened holds again once it lost the focus and gets it back (or was hidden)", async () => {
    timing.holdMs = 10_000;
    await picker(true, { userOpened: true });
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
    expect(rows().every((r) => held(r))).toBe(true);
    await picker(true, { userOpened: true });
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
    document.dispatchEvent(new Event("visibilitychange"));
    delete (document as unknown as Record<string, unknown>).visibilityState;
    expect(rows().every((r) => held(r))).toBe(true);
    rows()[0]!.click();
    await sleep(20);
    expect(answers()).toEqual([]);
  });

  test("a panel the page caused (no note) is held at open and by its first focus; a note that is not exactly {userOpened: true} changes nothing", async () => {
    timing.holdMs = 10_000;
    await picker(true);
    expect(rows().every((r) => held(r))).toBe(true);
    expect(rows().includes(document.activeElement as HTMLButtonElement)).toBe(false);
    for (const note of [{ userOpened: "true" }, { userOpened: 1 }, "userOpened", null, { held: false }]) notePanel!(note);
    window.dispatchEvent(new Event("focus"));
    expect(rows().every((r) => held(r))).toBe(true);
    rows()[0]!.click();
    rows()[0]!.focus();
    key("Enter");
    await sleep(20);
    expect(answers()).toEqual([]);
  });

  test("a separate window never listens for the note (it is always the background's)", async () => {
    await picker(false);
    expect(notePanel).toBeUndefined();
  });

  test("no rows (a save offer): Up and Down do nothing", async () => {
    dropListeners();
    await openPage(SAVE, {}, true);
    const before = document.activeElement;
    expect(key("ArrowDown")).toBe(false);
    expect(document.activeElement).toBe(before);
  });
});
