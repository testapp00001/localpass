import { beforeEach, describe, expect, test } from "bun:test";
import { findLoginForms, isOtpField, setValue } from "../src/content/detect";

const opts = { assumeVisible: true };

function page(html: string): Document {
  document.body.innerHTML = html;
  return document;
}

const field = (selector: string) => document.querySelector<HTMLInputElement>(selector);

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("finding login forms", () => {
  test("a plain email and password form", () => {
    page(`<form id="f"><input type="email" name="email"><input type="password" name="pw"><button>Sign in</button></form>`);
    const [form, ...rest] = findLoginForms(document, opts);
    expect(rest).toHaveLength(0);
    expect(form?.username).toBe(field('[name="email"]'));
    expect(form?.password).toBe(field('[name="pw"]'));
    expect(form?.isNewPassword).toBe(false);
  });

  test("a search box on the page is not the username", () => {
    page(`
      <header><input type="search" name="q" placeholder="Search"></header>
      <input type="text" name="site-search" placeholder="Search the site">
      <form><input type="text" name="login"><input type="password"></form>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.username).toBe(field('[name="login"]'));
  });

  test("autocomplete=username wins over other text boxes", () => {
    page(`<form>
      <input type="text" name="nickname">
      <input type="text" name="whatever" autocomplete="username">
      <input type="password">
    </form>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.username).toBe(field('[name="whatever"]'));
  });

  test("the box closest before the password is chosen", () => {
    page(`<form>
      <input type="text" name="first">
      <input type="text" name="second">
      <input type="password">
    </form>`);
    expect(findLoginForms(document, opts)[0]?.username).toBe(field('[name="second"]'));
  });

  test("a sign-up form with a repeated password", () => {
    page(`<form>
      <input type="email" name="email">
      <input type="password" name="pw1" autocomplete="new-password">
      <input type="password" name="pw2" autocomplete="new-password">
    </form>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.isNewPassword).toBe(true);
    expect(form?.password).toBe(field('[name="pw1"]'));
    expect(form?.extraPasswords).toEqual([field('[name="pw2"]')!]);
  });

  test("a change-password form without hints is still seen as new-password", () => {
    page(`<form><input type="password" name="current"><input type="password" name="new_password"><input type="password" name="confirm"></form>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.isNewPassword).toBe(true);
    expect(form?.password).toBe(field('[name="new_password"]'));
    expect(form?.extraPasswords).toHaveLength(2);
  });

  test("sign-in steps that ask for the account first", () => {
    page(`<form><input type="email" name="id" autocomplete="username"><button>Next</button></form>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.username).toBe(field('[name="id"]'));
    expect(form?.password).toBeNull();
  });

  test("a form with only a one-time code", () => {
    page(`<form><input type="text" name="otp" autocomplete="one-time-code" maxlength="6"></form>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.otp).toBe(field('[name="otp"]'));
    expect(form?.username).toBeNull();
    expect(form?.password).toBeNull();
  });

  test("a code box next to the password is found", () => {
    page(`<form><input type="text" name="user"><input type="password"><input type="text" name="verification_code" maxlength="6"></form>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.otp).toBe(field('[name="verification_code"]'));
    expect(form?.username).toBe(field('[name="user"]'));
  });

  test("inputs without a <form> element", () => {
    page(`<div class="login"><div><input type="text" name="user"></div><div><input type="password"></div><button>Go</button></div>
           <div class="other"><input type="text" name="elsewhere"></div>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.username).toBe(field('[name="user"]'));
    expect(form?.root.querySelector('[name="elsewhere"]')).toBeNull();
  });

  test("hidden and disabled boxes are ignored", () => {
    page(`<form>
      <input type="text" name="ghost" hidden>
      <input type="hidden" name="token">
      <input type="text" name="locked" disabled>
      <input type="text" name="real">
      <input type="password" style="display:none" name="decoy">
      <input type="password" name="visible">
    </form>`);
    const [form] = findLoginForms(document, opts);
    expect(form?.username).toBe(field('[name="real"]'));
    expect(form?.password).toBe(field('[name="visible"]'));
  });

  test("a newsletter box alone is not a login form", () => {
    page(`<form><input type="email" name="newsletter" placeholder="Your email"><button>Subscribe</button></form>`);
    expect(findLoginForms(document, opts)).toHaveLength(0);
  });

  test("two separate forms on one page", () => {
    page(`
      <form id="a"><input type="text" name="user_a"><input type="password" name="pw_a"></form>
      <form id="b"><input type="text" name="user_b"><input type="password" name="pw_b"></form>`);
    const forms = findLoginForms(document, opts);
    expect(forms).toHaveLength(2);
    expect(forms[0]?.username).toBe(field('[name="user_a"]'));
    expect(forms[1]?.password).toBe(field('[name="pw_b"]'));
  });

  test("nothing when there is no login", () => {
    page(`<p>Hello</p><input type="text" name="comment">`);
    expect(findLoginForms(document, opts)).toEqual([]);
  });
});

describe("one-time code fields", () => {
  test("recognises codes by autocomplete and by name", () => {
    page(`<input id="a" autocomplete="one-time-code"><input id="b" name="totp_code" maxlength="6"><input id="c" name="username" maxlength="6"><input id="d" name="code">`);
    expect(isOtpField(field("#a")!)).toBe(true);
    expect(isOtpField(field("#b")!)).toBe(true);
    expect(isOtpField(field("#c")!)).toBe(false);
    expect(isOtpField(field("#d")!)).toBe(false); // too open-ended without a length
  });
});

describe("filling a value", () => {
  test("goes through the native setter and tells the page", () => {
    page(`<input type="text" id="box">`);
    const box = field("#box")!;
    const seen: string[] = [];
    // A framework-style wrapper around `value` that must not be bypassed incorrectly.
    box.addEventListener("input", (e) => seen.push(`input:${(e.target as HTMLInputElement).value}`));
    box.addEventListener("change", (e) => seen.push(`change:${(e.target as HTMLInputElement).value}`));
    setValue(box, "alice@example.com");
    expect(box.value).toBe("alice@example.com");
    expect(seen).toEqual(["input:alice@example.com", "change:alice@example.com"]);
  });

  test("overwrites what was there", () => {
    page(`<input type="password" id="box" value="old">`);
    setValue(field("#box")!, "new");
    expect(field("#box")!.value).toBe("new");
  });
});
