import { describe, expect, test } from "bun:test";
import { createModalStack, wrapFocus } from "../src/lib/modalStack";

// Entries are numbers standing for their position in the document (higher = later = on top).
const byPosition = (a: number, b: number) => a > b;

describe("modal stack", () => {
  test("empty stack has no top", () => {
    const s = createModalStack<number>(byPosition);
    expect(s.top()).toBeUndefined();
    expect(s.size()).toBe(0);
  });

  test("the dialog later in the document is on top, whatever the opening order", () => {
    const s = createModalStack<number>(byPosition);
    s.open(5); // e.g. a browser pairing dialog, opened first
    s.open(1); // the verify dialog, opened later but earlier in the document
    expect(s.top()).toBe(5);
    const s2 = createModalStack<number>(byPosition);
    s2.open(1);
    s2.open(5);
    expect(s2.top()).toBe(5);
  });

  test("closing the top one exposes the next, and only it", () => {
    const s = createModalStack<number>(byPosition);
    const closeA = s.open(1);
    const closeB = s.open(2);
    const closeC = s.open(3);
    expect(s.top()).toBe(3);
    closeC();
    expect(s.top()).toBe(2);
    closeA();
    expect(s.top()).toBe(2);
    closeB();
    expect(s.top()).toBeUndefined();
  });

  test("a remove function acts once, even when the same entry is open twice", () => {
    const s = createModalStack<number>(byPosition);
    const first = s.open(4);
    s.open(4);
    first();
    first();
    expect(s.size()).toBe(1);
    expect(s.top()).toBe(4);
  });

  test("unknown order: the one opened last is on top", () => {
    const s = createModalStack<string>();
    s.open("pairing");
    s.open("verify");
    expect(s.top()).toBe("verify");
  });
});

describe("wrapFocus", () => {
  test("wraps at both ends and leaves the middle to the browser", () => {
    expect(wrapFocus(3, 2, false)).toBe(0);
    expect(wrapFocus(3, 0, true)).toBe(2);
    expect(wrapFocus(3, 1, false)).toBeUndefined();
    expect(wrapFocus(3, 1, true)).toBeUndefined();
    expect(wrapFocus(3, 0, false)).toBeUndefined();
  });

  test("focus outside the dialog moves into it", () => {
    expect(wrapFocus(3, -1, false)).toBe(0);
    expect(wrapFocus(3, -1, true)).toBe(2);
  });

  test("no focusable control: nothing to do", () => {
    expect(wrapFocus(0, -1, false)).toBeUndefined();
  });
});
