// The content script: finds login forms, puts the LocalPass badge in their fields, fills what the user
// chose in LocalPass's own panel, and notices sign-ins so they can be saved. It never sees a secret
// until the user picks a login in the panel (the background hands it over as the reply to this
// script's own `decide` request), never learns the titles or user names of saved logins, and the
// service worker decides which site a request is for.

import type { ChooseOutcome, DecisionRequest, PendingSaveRef, SiteStatus, ToContent } from "../shared/messages";
import type { LoginFill } from "../shared/protocol";
import { MAX_CAPTURE_CHARS } from "../shared/limits";
import { ask, askDecision } from "./ask";
import { type LoginForm, findLoginForms, setValue } from "./detect";
import { conditionalPending, conditionalRequest, installPasskeyBridge, resolveConditional } from "./passkeys";
import { hideBadge, showBadge } from "./ui";

// Listen for the page's passkey requests from the very start (this script runs at document_start).
installPasskeyBridge();

let forms: LoginForm[] = [];
const decorated = new WeakSet<HTMLInputElement>();

let lastRescan = 0;

function formOf(field: Element | null): LoginForm | undefined {
  if (!field) return undefined;
  const known = forms.find((f) => f.root.contains(field));
  if (known) return known;
  // The page may have changed since the last (debounced) scan, e.g. a sign-in submitted right away.
  // Most clicks and keys are outside any login form, so don't rescan a heavy page on every one.
  const now = Date.now();
  if (now - lastRescan < 250) return undefined;
  lastRescan = now;
  scan();
  return forms.find((f) => f.root.contains(field));
}

// ---- filling ----

function applyFill(form: LoginForm, fill: LoginFill): void {
  if (form.username && fill.username) setValue(form.username, fill.username);
  if (form.password && fill.password) setValue(form.password, fill.password);
  if (form.otp && fill.totpCode) setValue(form.otp, fill.totpCode);
}

/** The form to fill when the popup asks: the one with focus, else the first with a password. */
function bestForm(): LoginForm | undefined {
  return formOf(document.activeElement) ?? forms.find((f) => f.password) ?? forms[0];
}

// ---- the badge and the picker ----

/**
 * The badge asks for LocalPass's picker (the toolbar panel); the user chooses there, and the outcome comes
 * back as the reply: a login fills this particular form, a generated password its new-password boxes, and a
 * passkey answers the page's pending passkey request (Chrome, conditional mediation).
 */
async function chooseInPicker(form: LoginForm): Promise<void> {
  const ref = crypto.randomUUID();
  const origin = location.origin;
  const withdraw = () => void ask({ kind: "decideCancel", ref }).catch(() => {});
  window.addEventListener("pagehide", withdraw, { once: true });
  // Hints the page could fake: the worst outcome is a "Generate" row or the page's own passkey request listed.
  const request: DecisionRequest = { decision: "chooseLogin" };
  if (form.isNewPassword) request.newPassword = true;
  const pendingPasskey = conditionalRequest();
  if (pendingPasskey) request.passkeys = pendingPasskey.params;
  try {
    const outcome = await askDecision<ChooseOutcome>(ref, request);
    if ("passkey" in outcome) {
      if (pendingPasskey) resolveConditional(pendingPasskey.id, outcome.passkey);
      return;
    }
    if (location.origin !== origin || !form.root.isConnected) return;
    if ("fill" in outcome) applyFill(form, outcome.fill);
    else if ("generated" in outcome) for (const box of [form.password, ...form.extraPasswords]) if (box) setValue(box, outcome.generated);
  } catch {
    // Cancelled, or unavailable. The normal toolbar popup remains available.
  } finally {
    window.removeEventListener("pagehide", withdraw);
  }
}

function onFocus(field: HTMLInputElement, form: LoginForm): void {
  void ask<SiteStatus>({ kind: "findLogins", interactive: false })
    .then((reply) => {
      if (document.activeElement !== field) return;
      const useful = reply.status === "ok" && (reply.hasLogins || form.isNewPassword || conditionalPending());
      // Muted when LocalPass can't help yet (locked or not connected); the panel explains why.
      showBadge(field, !useful, () => void chooseInPicker(form));
    })
    .catch(() => {});
}

function scan(): void {
  forms = findLoginForms(document);
  for (const form of forms) {
    for (const field of [form.username, form.password, form.otp]) {
      if (!field || decorated.has(field)) continue;
      decorated.add(field);
      field.addEventListener("focus", () => onFocus(field, form));
      field.addEventListener("blur", () => setTimeout(() => !document.hasFocus() || hideBadgeIfAway(), 200));
      if (document.activeElement === field) onFocus(field, form);
    }
  }
}

function hideBadgeIfAway(): void {
  const active = document.activeElement;
  if (!(active instanceof HTMLInputElement) || !formOf(active)) hideBadge();
}

// ---- noticing sign-ins ----

let lastCapture = "";
let lastCaptureByUser = false;

let lastSeenUser = "";

/**
 * Whether the DOM event behind a capture shows that the person sent the sign-in: a trusted click
 * or a trusted Enter key. A submit event does not count: `form.requestSubmit()` and
 * `button.click()` from a page script fire a submit event with `isTrusted === true` (measured in
 * Firefox 160 and Chrome 157), so a page could fake it. A capture sent by the user may make the
 * worker connect to the app; any other capture never does.
 */
function sentByUser(e: Event): boolean {
  return e.isTrusted === true && e.type !== "submit";
}

function capture(form: LoginForm, byUser: boolean): void {
  const password = form.password?.value ?? "";
  if (!form.password) return seenUser(form);
  if (!password) return;
  const username = form.username?.value ?? "";
  if (username.length > MAX_CAPTURE_CHARS || password.length > MAX_CAPTURE_CHARS) return; // not a sign-in a person typed
  const key = `${username}\0${password}`;
  // The same sign-in once, except that a page's own earlier submit does not hide the user's.
  if (key === lastCapture && (lastCaptureByUser || !byUser)) return;
  lastCapture = key;
  lastCaptureByUser = byUser;
  void ask({ kind: "capture", username, password, byUser }).catch(() => {});
  // Single-page sites don't navigate: if the form is gone a moment later, the sign-in worked.
  if (window === window.top) {
    setTimeout(() => {
      if (form.password && !form.password.isConnected) void checkPending();
    }, 1500);
  }
}

/**
 * The first step of a two-step sign-in (an account name, the password on the next page): the
 * worker keeps the name for the password step, which has no user name box of its own. This never
 * makes the worker connect, whoever sent it: the same kind of form is a newsletter box with
 * `autocomplete="email"`, and signing up for one must not start the app. The password step's
 * capture connects when the user sends it.
 */
function seenUser(form: LoginForm): void {
  const username = form.username?.value ?? "";
  if (!username.trim() || username.length > MAX_CAPTURE_CHARS || username === lastSeenUser) return;
  lastSeenUser = username;
  void ask({ kind: "seenUser", username }).catch(() => {});
}

function watchSubmissions(): void {
  document.addEventListener(
    "submit",
    (e) => {
      const form = formOf(e.target as Element);
      if (form) capture(form, sentByUser(e));
    },
    true,
  );
  document.addEventListener(
    "click",
    (e) => {
      const button = (e.target as Element | null)?.closest?.('button, input[type="submit"], [role="button"]');
      const form = formOf(button ?? null);
      if (form) capture(form, sentByUser(e));
    },
    true,
  );
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key !== "Enter") return;
      const form = formOf(e.target as Element);
      if (form) capture(form, sentByUser(e));
    },
    true,
  );
}

async function checkPending(): Promise<void> {
  if (window !== window.top) return;
  let pending: PendingSaveRef | null = null;
  try {
    pending = await ask<PendingSaveRef | null>({ kind: "pendingSave" });
  } catch {
    return;
  }
  if (pending) offerSave(pending);
}

/**
 * The offer is shown in LocalPass's panel, never in the page. Every page of the tab asks while the capture
 * lives: the first opens the panel, later ones take the open offer over (after the redirects of a sign-in),
 * so the background stays awake for it.
 */
function offerSave(pending: PendingSaveRef): void {
  void askDecision(crypto.randomUUID(), { decision: "saveLogin", id: pending.id }).catch(() => {});
}

// ---- wiring ----

chrome.runtime.onMessage.addListener((message: ToContent) => {
  if (message.kind === "doFill") {
    // The service worker checked the tab, but the page may have changed since: fill only the site
    // the password was fetched for, and only in the top page.
    if (window !== window.top || message.origin !== location.origin) return false;
    const form = bestForm();
    if (form) applyFill(form, message.fill);
  }
  return false;
});

let scanTimer: ReturnType<typeof setTimeout> | undefined;
new MutationObserver(() => {
  clearTimeout(scanTimer);
  scanTimer = setTimeout(scan, 300);
}).observe(document.documentElement, { childList: true, subtree: true });

scan();
watchSubmissions();
// This script starts before the page is parsed: look for forms and offer a pending save once the
// document is there.
const ready = () => {
  scan();
  void checkPending();
};
if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", ready, { once: true });
else ready();
