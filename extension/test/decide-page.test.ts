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

async function openPage(info: Json) {
  installChrome();
  sent = [];
  answers = { decisionInfo: () => ({ ok: true, data: info }) };
  const chromeApi = (globalThis as unknown as { chrome: { runtime: Json } }).chrome;
  chromeApi.runtime.connect = () => ({});
  chromeApi.runtime.sendMessage = async (message: Json) => {
    sent.push(message);
    return answers[message.kind as string]?.(message) ?? { ok: true, data: {} };
  };
  document.body.innerHTML = HTML;
  happy().setURL("moz-extension://uuid/decide.html?id=abc");
  await import(`../src/decide/index.ts?page=${copies++}`);
  await sleep(10);
}

const main = () => document.getElementById("main")!;
const buttons = () => [...main().querySelectorAll("button")];
const button = (label: string) => buttons().find((b) => b.textContent === label)!;

beforeEach(() => {
  savedUrl = window.location.href;
  // Tests act at once and do not wait for pings (the tests of the rules below turn them on).
  timing.holdMs = 0;
  timing.keepAliveMs = 3_600_000;
});
afterEach(() => {
  happy().setURL(savedUrl);
  document.body.innerHTML = "";
});

const CREATE_INFO = {
  decision: "passkeyCreate",
  origin: "https://login.example.com",
  host: "login.example.com",
  request: { rpId: "example.com", rpName: "<img src=x onerror=alert(1)>", userName: "alice", userDisplayName: "Alice", userHandle: "AA", challenge: "AAAA", algorithms: [-7], exclude: [], uv: "preferred" },
};

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
      if (m.kind === "unlock") return ((locked = false), { ok: true, data: {} });
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
  test("action buttons and rows are disabled for the hold, safe buttons are not, and nothing that acts has focus", async () => {
    timing.holdMs = 120;
    await openPage(CREATE_INFO);
    expect(button("Create passkey").disabled).toBe(true);
    expect(button("Use another method").disabled).toBe(false);
    expect(button("Cancel").disabled).toBe(false);
    expect(document.activeElement?.textContent).not.toBe("Create passkey");
    button("Create passkey").click(); // a key or click that lands early
    await sleep(30);
    expect(sent.some((m) => m.kind === "decisionAnswer")).toBe(false);
    await until(() => !button("Create passkey").disabled);
    button("Create passkey").click();
    await until(() => sent.some((m) => m.kind === "decisionAnswer"));
    expect(sent.at(-1)).toMatchObject({ answer: { action: "create" } });
  });

  test("coming back to the window starts the wait again", async () => {
    timing.holdMs = 60;
    await openPage(CREATE_INFO);
    await until(() => !button("Create passkey").disabled);
    window.dispatchEvent(new Event("focus"));
    expect(button("Create passkey").disabled).toBe(true);
    await until(() => !button("Create passkey").disabled);
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
    expect(main().querySelector<HTMLButtonElement>(".row")!.disabled).toBe(true);
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
