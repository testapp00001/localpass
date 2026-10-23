// V6, page side: a fill is applied only when it was fetched for this page's own site.

import { beforeAll, describe, expect, test } from "bun:test";
import { type FakeChrome, installChrome } from "./chrome";

let fake: FakeChrome;
const sent: { kind?: string; [key: string]: unknown }[] = [];

beforeAll(async () => {
  (window as unknown as { happyDOM: { setURL(u: string): void } }).happyDOM.setURL("https://a.example/login");
  // happy-dom has no layout: give every element a size so the form counts as visible.
  (Element.prototype as unknown as { getBoundingClientRect: () => unknown }).getBoundingClientRect = () => ({ x: 0, y: 0, top: 0, left: 0, right: 100, bottom: 20, width: 100, height: 20 });
  document.body.innerHTML = `<form><input name="username" autocomplete="username"><input name="password" type="password"></form>`;
  fake = installChrome();
  (globalThis as unknown as { chrome: { runtime: { sendMessage: unknown } } }).chrome.runtime.sendMessage = async (message: { kind?: string }) => {
    sent.push(message);
    return { ok: true, data: null };
  };
  await import("../src/content/index");
});

const value = (name: string) => (document.querySelector(`input[name=${name}]`) as HTMLInputElement).value;
const FILL = { id: "i", username: "alice", password: "pw", totpCode: null };
const deliver = (message: unknown) => fake.onMessage!(message, {}, () => {});

describe("doFill", () => {
  test("a fill fetched for another site is ignored", () => {
    deliver({ kind: "doFill", fill: FILL, origin: "https://evil.example" });
    expect(value("username")).toBe("");
    expect(value("password")).toBe("");
  });

  test("a fill without a site is ignored", () => {
    deliver({ kind: "doFill", fill: FILL });
    expect(value("password")).toBe("");
  });

  test("a fill for this site is applied", () => {
    deliver({ kind: "doFill", fill: FILL, origin: "https://a.example" });
    expect(value("username")).toBe("alice");
    expect(value("password")).toBe("pw");
  });
});

describe("capturing a sign-in to offer saving (RA-3)", () => {
  const submit = () => document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  const set = (name: string, text: string) => ((document.querySelector(`input[name=${name}]`) as HTMLInputElement).value = text);
  const captures = () => sent.filter((m) => m.kind === "capture");

  test("a typed sign-in is sent", () => {
    sent.length = 0;
    set("username", "bob");
    set("password", "bob-secret");
    submit();
    expect(captures()).toEqual([{ kind: "capture", username: "bob", password: "bob-secret" }]);
  });

  test("a page that sets a huge value gets nothing sent", () => {
    sent.length = 0;
    set("username", "bob");
    set("password", "x".repeat(5_000_000));
    submit();
    set("password", "y");
    set("username", "z".repeat(5_000));
    submit();
    expect(captures()).toEqual([]);
  });
});
