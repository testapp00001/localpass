// The page interface must not react to clicks the user could not see (RA-1, clickjacking).
// Real hit-testing and occlusion need a browser: e2e/run.ts covers those. These tests pin the rules.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { FakeVisibility } from "./setup";

// A fresh copy of the module per test: other test files replace the global DOM (rp.test.ts), which a
// module that remembers its host element and observer would not survive.
let ui: typeof import("../src/content/ui");
let copies = 0;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function shadowRoot(): ShadowRoot {
  const host = [...document.documentElement.children].filter((e) => e.shadowRoot).at(-1);
  return host!.shadowRoot!;
}
const find = <T extends Element>(selector: string) => shadowRoot().querySelector(selector) as T;

function field(): HTMLInputElement {
  const input = document.createElement("input");
  document.body.append(input);
  return input;
}

beforeEach(async () => {
  (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = FakeVisibility;
  FakeVisibility.visible = true;
  FakeVisibility.observers = [];
  ui = await import(`../src/content/ui.ts?copy=${copies++}`);
  ui.guard.holdMs = 0;
  ui.guard.needTrusted = false;
});
afterEach(() => {
  ui.hideMenu();
  ui.hideBadge();
  document.body.replaceChildren();
});

function menuWith(onSelect: () => void) {
  ui.showMenu(field(), { title: "t", rows: [{ label: "Local site", onSelect }] });
  return find<HTMLButtonElement>(".row");
}

describe("the host element", () => {
  test("every declaration that keeps it in place is inline !important", () => {
    menuWith(() => {});
    const host = [...document.documentElement.children].filter((e) => e.shadowRoot).at(-1) as HTMLElement;
    for (const name of ["all", "position", "top", "left", "width", "height", "z-index"]) {
      expect(host.style.getPropertyPriority(name)).toBe("important");
    }
    expect(host.style.getPropertyValue("z-index")).toBe("2147483647");
  });
});

describe("a click on a control", () => {
  test("is ignored until the browser reports the control plainly visible", () => {
    FakeVisibility.visible = false;
    let picked = 0;
    const row = menuWith(() => picked++);
    row.click();
    expect(picked).toBe(0);
    expect(row.hasAttribute("data-ready")).toBe(false);
    FakeVisibility.report(row, true);
    expect(row.hasAttribute("data-ready")).toBe(true);
    row.click();
    expect(picked).toBe(1);
  });

  test("is ignored again when something hides, covers or transforms the control", () => {
    let picked = 0;
    const row = menuWith(() => picked++);
    FakeVisibility.report(row, false);
    expect(row.hasAttribute("data-ready")).toBe(false);
    row.click();
    expect(picked).toBe(0);
    FakeVisibility.report(row, true);
    row.click();
    expect(picked).toBe(1);
  });

  test("waits holdMs after the control became visible, and starts over if it was hidden meanwhile", async () => {
    ui.guard.holdMs = 60;
    let picked = 0;
    const row = menuWith(() => picked++);
    row.click();
    expect(picked).toBe(0);
    await sleep(30);
    FakeVisibility.report(row, false); // covered for a moment: the wait starts over
    FakeVisibility.report(row, true);
    await sleep(40);
    row.click();
    expect(picked).toBe(0);
    await sleep(60);
    expect(row.hasAttribute("data-ready")).toBe(true);
    row.click();
    expect(picked).toBe(1);
  });

  test("made up by a script (not trusted) is ignored", () => {
    ui.guard.needTrusted = true;
    let picked = 0;
    const row = menuWith(() => picked++);
    row.click();
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(picked).toBe(0);
  });

  test("on the sheet: the primary button needs a visible control, cancelling and handing back do not", () => {
    FakeVisibility.visible = false;
    const sheet = ui.showSheet("Create a passkey?", () => {});
    const calls: string[] = [];
    sheet.setButtons([
      { label: "Create passkey", primary: true, onClick: () => calls.push("create") },
      { label: "Use another method", onClick: () => calls.push("other") },
    ]);
    const [create, other] = [...shadowRoot().querySelectorAll<HTMLButtonElement>(".sheet .btn")];
    create!.click();
    other!.click();
    expect(calls).toEqual(["other"]);
    FakeVisibility.report(create!, true);
    create!.click();
    expect(calls).toEqual(["other", "create"]);
    sheet.close();
  });

  test("on the sheet: a passkey row needs a visible control", () => {
    FakeVisibility.visible = false;
    const sheet = ui.showSheet("Sign in with a passkey", () => {});
    let picked = 0;
    sheet.setRows([{ label: "alice", onSelect: () => picked++ }]);
    const row = find<HTMLButtonElement>(".sheet .row");
    row.click();
    expect(picked).toBe(0);
    FakeVisibility.report(row, true);
    row.click();
    expect(picked).toBe(1);
    sheet.close();
  });
});

describe("the badge", () => {
  test("ignores clicks while it is still moving, and while it is not visible", async () => {
    ui.guard.holdMs = 50;
    const input = field();
    let opened = 0;
    let right = 300;
    input.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, right, bottom: 30, width: right, height: 30, toJSON() {} }) as DOMRect;
    ui.showBadge(input, false, () => opened++);
    const badge = find<HTMLButtonElement>(".badge");
    badge.click(); // just placed and not yet reported visible
    expect(opened).toBe(0);
    await sleep(80);
    badge.click();
    expect(opened).toBe(1);
    // The page moves the field under the pointer: the badge follows, and holds still clicks off.
    right = 340;
    await sleep(40); // the badge follows within a frame or two
    expect(badge.style.left).toBe("312px");
    badge.click();
    expect(opened).toBe(1);
    await sleep(70);
    badge.click();
    expect(opened).toBe(2);
  });
});

describe("a browser without visibility tracking", () => {
  test("leaves the controls inert instead of trusting every click", async () => {
    const saved = (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver;
    (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = undefined;
    try {
      const fresh = await import(`../src/content/ui.ts?unsupported=${Date.now()}`);
      fresh.guard.holdMs = 0;
      fresh.guard.needTrusted = false;
      let picked = 0;
      fresh.showMenu(field(), { title: "t", rows: [{ label: "x", onSelect: () => picked++ }] });
      const host = [...document.documentElement.children].filter((e) => e.shadowRoot).at(-1)!;
      const row = host.shadowRoot!.querySelector(".row") as HTMLButtonElement;
      row.click();
      expect(picked).toBe(0);
      expect(row.hasAttribute("data-ready")).toBe(false);
      fresh.hideMenu();
    } finally {
      (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = saved;
    }
  });
});
