// What LocalPass still draws into a web page: only the badge in a login field, whose click merely asks for
// LocalPass's own panel (8.3b). A page that hides, covers or restyles it can at most get the panel opened;
// e2e/run.ts shows that in a real browser (`/attack-style`, `/attack-cover`).

import { afterEach, beforeEach, describe, expect, test } from "bun:test";

// A fresh copy of the module per test: other test files replace the global DOM (rp.test.ts), which a
// module that remembers its host element would not survive.
let ui: typeof import("../src/content/ui");
let copies = 0;

function shadowRoot(): ShadowRoot {
  const host = [...document.documentElement.children].filter((e) => e.shadowRoot).at(-1);
  return host!.shadowRoot!;
}

function field(): HTMLInputElement {
  const input = document.createElement("input");
  document.body.append(input);
  return input;
}

beforeEach(async () => {
  for (const leftover of [...document.documentElement.children]) if (leftover.shadowRoot) leftover.remove();
  ui = await import(`../src/content/ui.ts?copy=${copies++}`);
  ui.guard.needTrusted = false;
});
afterEach(() => {
  ui.hideBadge();
  document.body.replaceChildren();
});

describe("the host element", () => {
  test("every declaration that keeps it in place is inline !important", () => {
    ui.showBadge(field(), false, () => {});
    const host = [...document.documentElement.children].filter((e) => e.shadowRoot).at(-1) as HTMLElement;
    for (const name of ["all", "position", "top", "left", "width", "height", "z-index"]) {
      expect(host.style.getPropertyPriority(name)).toBe("important");
    }
    expect(host.style.getPropertyValue("z-index")).toBe("2147483647");
  });
});

describe("the badge", () => {
  test("a click a script made up is ignored; a real one only calls the opener", () => {
    ui.guard.needTrusted = true;
    let opened = 0;
    ui.showBadge(field(), false, () => opened++);
    const badge = shadowRoot().querySelector(".badge") as HTMLButtonElement;
    badge.click();
    badge.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(opened).toBe(0);
    // happy-dom cannot make trusted events: one the browser reports as real.
    const real = new MouseEvent("click", { bubbles: true, cancelable: true });
    Object.defineProperty(real, "isTrusted", { value: true });
    badge.dispatchEvent(real);
    expect(opened).toBe(1);
    // The page under it never sees the click.
    expect(real.defaultPrevented).toBe(true);
  });

  test("it is the only thing LocalPass draws: no menu, save bar or dialog exists in a page any more", () => {
    ui.showBadge(field(), true, () => {});
    const nodes = [...shadowRoot().children].map((e) => `${e.tagName.toLowerCase()}${e.className ? `.${e.className.replace(/ /g, ".")}` : ""}`);
    expect(nodes).toEqual(["style", "button.badge.muted"]);
    expect(Object.keys(ui).sort()).toEqual(["guard", "hideBadge", "showBadge"]);
  });

  test("the same field again keeps its button, so a click already under way is not lost", () => {
    // The field's focus event fires again when its window or tab gets the focus back, often from the click on the
    // badge itself; measured in headless Firefox 160 (8.6), the badge was rebuilt 7 ms before that click's mousedown.
    const input = field();
    const calls: string[] = [];
    ui.showBadge(input, true, () => calls.push("first"));
    const before = shadowRoot().querySelector(".badge") as HTMLButtonElement;
    ui.showBadge(input, false, () => calls.push("second"));
    const buttons = shadowRoot().querySelectorAll(".badge");
    expect(buttons.length).toBe(1);
    expect(buttons[0]).toBe(before);
    expect(before.isConnected).toBe(true);
    expect(before.className).toBe("badge");
    before.click();
    expect(calls).toEqual(["second"]);
  });

  test("another field gets a new button and the old one goes away", () => {
    const calls: string[] = [];
    ui.showBadge(field(), false, () => calls.push("first"));
    const first = shadowRoot().querySelector(".badge") as HTMLButtonElement;
    ui.showBadge(field(), true, () => calls.push("second"));
    const buttons = shadowRoot().querySelectorAll(".badge");
    expect(buttons.length).toBe(1);
    expect(buttons[0]).not.toBe(first);
    expect(first.isConnected).toBe(false);
    expect((buttons[0] as HTMLElement).className).toBe("badge muted");
    (buttons[0] as HTMLButtonElement).click();
    expect(calls).toEqual(["second"]);
  });

  test("it follows its field and goes away with it", async () => {
    const input = field();
    let right = 300;
    input.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, top: 0, right, bottom: 30, width: right, height: 30, toJSON() {} }) as DOMRect;
    ui.showBadge(input, false, () => {});
    const badge = shadowRoot().querySelector(".badge") as HTMLButtonElement;
    expect(badge.style.left).toBe("272px");
    right = 340;
    await new Promise((r) => setTimeout(r, 50));
    expect(badge.style.left).toBe("312px");
    input.remove();
    await new Promise((r) => setTimeout(r, 50));
    expect(shadowRoot().querySelector(".badge")).toBeNull();
  });
});
