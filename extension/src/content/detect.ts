// Finding login forms on a page. Pure functions over the DOM, so they can be tested on fixtures.

export interface LoginForm {
  /** The element that groups the fields (a <form> or the closest sensible container). */
  root: Element;
  username: HTMLInputElement | null;
  /** The password the user types to sign in (or, on a sign-up form, the new password). */
  password: HTMLInputElement | null;
  /** More password boxes: "repeat password" on sign-up and change-password forms. */
  extraPasswords: HTMLInputElement[];
  /** A field for a one-time code, if the form has one. */
  otp: HTMLInputElement | null;
  /** The form asks for a new password (sign-up or password change). */
  isNewPassword: boolean;
}

export interface DetectOptions {
  /** Treat elements as visible without measuring them (for tests without layout). */
  assumeVisible?: boolean;
}

const USERNAME_HINT = /user|login|e-?mail|account|identifier|\bid\b|phone|member/i;
const SEARCH_HINT = /search|query|find|filter/i;
const OTP_HINT = /otp|one.?time|2fa|two.?factor|totp|authenticator|verification|security.?code|passcode|\bcode\b|token/i;
const NEW_PASSWORD_HINT = /new|confirm|repeat|retype|again/i;

function attr(el: Element, name: string): string {
  return el.getAttribute(name) ?? "";
}

function hintText(el: HTMLInputElement): string {
  return [attr(el, "name"), el.id, attr(el, "placeholder"), attr(el, "aria-label"), attr(el, "autocomplete")].join(" ");
}

function autocompleteTokens(el: HTMLInputElement): string[] {
  return attr(el, "autocomplete").toLowerCase().split(/\s+/).filter(Boolean);
}

export function isVisible(el: HTMLElement, options: DetectOptions = {}): boolean {
  if (el.hidden) return false;
  const view = el.ownerDocument.defaultView;
  const style = view?.getComputedStyle(el);
  if (style && (style.display === "none" || style.visibility === "hidden")) return false;
  if (options.assumeVisible) return true;
  const rect = el.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
}

/** A box the user can type into (not hidden, disabled or read-only). */
export function isFillable(el: HTMLInputElement, options: DetectOptions = {}): boolean {
  return !el.disabled && !el.readOnly && el.type !== "hidden" && isVisible(el, options);
}

function isTextual(el: HTMLInputElement): boolean {
  return ["text", "email", "tel", "url", "number", ""].includes(el.type.toLowerCase());
}

function isUsernameCandidate(el: HTMLInputElement): boolean {
  if (!isTextual(el)) return false;
  const tokens = autocompleteTokens(el);
  if (tokens.includes("one-time-code")) return false;
  if (SEARCH_HINT.test(hintText(el)) && !tokens.includes("username")) return false;
  return true;
}

export function isOtpField(el: HTMLInputElement): boolean {
  if (!["text", "tel", "number", "password", ""].includes(el.type.toLowerCase())) return false;
  if (autocompleteTokens(el).includes("one-time-code")) return true;
  const max = Number(attr(el, "maxlength"));
  const shortField = max >= 4 && max <= 10;
  return shortField && OTP_HINT.test(hintText(el)) && !USERNAME_HINT.test(attr(el, "name") + el.id);
}

function isNewPasswordField(el: HTMLInputElement): boolean {
  return autocompleteTokens(el).includes("new-password") || NEW_PASSWORD_HINT.test(attr(el, "name") + " " + el.id);
}

/** Position of `a` relative to `b` in the document. */
function before(a: Element, b: Element): boolean {
  return !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
}

/** The group of fields around `anchor`: its <form>, or a nearby container with other inputs. */
function containerOf(anchor: HTMLInputElement): Element {
  if (anchor.form) return anchor.form;
  let node: Element = anchor;
  for (let depth = 0; depth < 5 && node.parentElement; depth++) {
    node = node.parentElement;
    if (node.querySelectorAll("input").length > 1) return node;
  }
  return anchor.parentElement ?? anchor;
}

function pickUsername(root: Element, password: HTMLInputElement, options: DetectOptions): HTMLInputElement | null {
  const candidates = [...root.querySelectorAll<HTMLInputElement>("input")].filter(
    (el) => el !== password && isUsernameCandidate(el) && isFillable(el, options),
  );
  const preceding = candidates.filter((el) => before(el, password));
  const pool = preceding.length ? preceding : candidates;
  if (!pool.length) return null;
  const score = (el: HTMLInputElement) => {
    let s = 0;
    const tokens = autocompleteTokens(el);
    if (tokens.includes("username")) s += 6;
    if (tokens.includes("email")) s += 4;
    if (el.type === "email") s += 3;
    if (USERNAME_HINT.test(hintText(el))) s += 2;
    // Closer to the password box wins.
    s += preceding.indexOf(el) + 1 > 0 ? (preceding.indexOf(el) + 1) / preceding.length : 0;
    return s;
  };
  return pool.reduce((best, el) => (score(el) > score(best) ? el : best));
}

/** Every login-like form in the document. */
export function findLoginForms(doc: Document, options: DetectOptions = {}): LoginForm[] {
  const forms: LoginForm[] = [];
  const claimed = new Set<Element>();
  const passwords = [...doc.querySelectorAll<HTMLInputElement>('input[type="password"]')].filter((el) => isFillable(el, options));

  for (const password of passwords) {
    if (claimed.has(password)) continue;
    const root = containerOf(password);
    const sameRoot = passwords.filter((p) => !claimed.has(p) && containerOf(p) === root);
    sameRoot.forEach((p) => claimed.add(p));
    const [first, ...rest] = sameRoot;
    if (!first) continue;
    // With several password boxes the form is a sign-up or a password change.
    const isNew = sameRoot.some(isNewPasswordField) || sameRoot.length >= 2;
    const main = isNew ? (sameRoot.find(isNewPasswordField) ?? first) : first;
    const otherBoxes = sameRoot.filter((p) => p !== main);
    forms.push({
      root,
      username: pickUsername(root, main, options),
      password: main,
      extraPasswords: isNew ? otherBoxes : rest,
      otp: findOtp(root, options),
      isNewPassword: isNew,
    });
  }

  // Sign-in that asks for the account first and the password on the next step.
  const identifiers = [...doc.querySelectorAll<HTMLInputElement>("input")].filter(
    (el) => isTextual(el) && isFillable(el, options) && !claimed.has(el) && isIdentifierField(el),
  );
  for (const el of identifiers) {
    const root = containerOf(el);
    if (forms.some((f) => f.root.contains(el))) continue;
    forms.push({ root, username: el, password: null, extraPasswords: [], otp: findOtp(root, options), isNewPassword: false });
  }

  // Pages that only ask for a 2FA code.
  for (const el of doc.querySelectorAll<HTMLInputElement>("input")) {
    if (!isOtpField(el) || !isFillable(el, options) || forms.some((f) => f.root.contains(el))) continue;
    forms.push({ root: containerOf(el), username: null, password: null, extraPasswords: [], otp: el, isNewPassword: false });
  }
  return forms;
}

function isIdentifierField(el: HTMLInputElement): boolean {
  const tokens = autocompleteTokens(el);
  return tokens.includes("username") || (tokens.includes("email") && !tokens.includes("new-password"));
}

function findOtp(root: Element, options: DetectOptions): HTMLInputElement | null {
  return [...root.querySelectorAll<HTMLInputElement>("input")].find((el) => isOtpField(el) && isFillable(el, options)) ?? null;
}

/** Sets a field's value the way a user typing would, so frameworks (React, Vue) notice. */
export function setValue(el: HTMLInputElement, value: string): void {
  const setter =
    Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), "value")?.set ??
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  el.dispatchEvent(new FocusEvent("focus"));
  if (setter) setter.call(el, value);
  else el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
  el.dispatchEvent(new Event("change", { bubbles: true }));
}
