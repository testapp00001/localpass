// The decision page, in every browser: in the toolbar panel, or a separate window where the browser cannot open the
// panel without a click. It is the only place where the user picks a login (or a generated password, or a passkey for
// a page's pending sign-in), saves one, or confirms a passkey. The background opened it for one request and tells it
// what the request is (`decisionInfo`); the user decides here, and the background carries the answer out and hands
// any secret to the page's content script, never to this page. Everything the page supplied is shown only as text,
// through `displayText`, after the site the browser reported.
// A panel can be closed and shown again (toolbar icon) while a save or passkey request waits: each showing
// starts afresh from `decisionInfo`, with the same hold on every acting control (none when the user opened it).
// Keys: Up and Down move between the rows, Enter or Space acts on the focused one, Esc closes.

import { displayText } from "../shared/limits";
import type { DecisionAnswer, DecisionDone, DecisionInfo, FindLoginsReply, PanelNote, Reply, ToBackground } from "../shared/messages";
import { userCheckName } from "../shared/platform";
import { BrokerError, type PasskeyCandidate } from "../shared/protocol";
import { createDialogLines, describePasskey, saveOffer } from "../shared/shown";
import { timing } from "./timing";

const id = new URLSearchParams(location.search).get("id") ?? "";
const panel = new URLSearchParams(location.search).get("presentation") === "panel";
document.body.classList.toggle("panel", panel);
const main = document.getElementById("main") as HTMLElement;
const siteLabel = document.getElementById("site") as HTMLElement;

/** Codes that mean LocalPass cannot be used right now: a passkey request then goes back to the browser. */
const UNAVAILABLE = new Set(["unpaired", "appNotRunning", "disconnected", "denied", "untrusted", "error", "noVault", "offline", "timeout"]);

async function ask<T>(message: ToBackground): Promise<T> {
  const reply = (await chrome.runtime.sendMessage(message)) as Reply<T> | undefined;
  if (!reply) throw new BrokerError("disconnected", "LocalPass did not answer");
  if (!reply.ok) throw new BrokerError(reply.code, reply.message);
  return reply.data;
}

/** Pings stop while an answer is being carried out and after it worked (the window is about to close). */
let paused = false;
const answer = (a: DecisionAnswer) => {
  paused = true;
  return ask<DecisionDone>({ kind: "decisionAnswer", id, answer: a }).then((done) => {
    if (panel && !done.message) window.close();
    return done;
  });
};
const code = (e: unknown) => (e instanceof BrokerError ? e.code : "internal");
const text = (e: unknown) => displayText(e instanceof Error ? e.message : String(e), 200);

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", content?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content !== undefined) node.textContent = content;
  return node;
}

interface Button {
  label: string;
  primary?: boolean;
  /** Cancelling or handing the request back cannot do harm, so it works at once. */
  safe?: boolean;
  /** What Esc (and a click outside, for some decisions) does in this view: the button that cancels or closes. */
  dismiss?: boolean;
  onClick: () => void;
}
interface Row {
  label: string;
  sub?: string;
  onSelect: () => void;
}

/**
 * The hold: for `timing.holdMs` after each view and each return to the window, the controls that act for the user
 * (everything but the safe buttons) do nothing, whether clicked or pressed with Enter or Space. They are not greyed
 * out (`disabled` would dim them); they say so to assistive technology with `aria-disabled`.
 *
 * A panel the user opened (toolbar icon or shortcut) skips the hold: the background tells it so over its port
 * (`PanelNote.userOpened`), and only when it did not itself ask the browser to open that panel. Nothing a page or
 * a content script sends can say so. Until then, and in every panel a page caused, the hold applies. Once the panel
 * has lost the focus (or was hidden), coming back holds again, also in a panel the user opened: the user may have
 * been typing elsewhere. Its own first focus as it opens (measured: every panel gets one) is not a return.
 */
let guarded: HTMLButtonElement[] = [];
/** What each acting control does, for a click and for Enter or Space on it. */
const actions = new WeakMap<EventTarget, () => void>();
let held = false;
let holdTimer: ReturnType<typeof setTimeout> | undefined;
/** The background said the user opened this panel (never set in a separate window). */
let userOpened = false;
/** The panel lost the focus or was hidden at least once since it opened. */
let away = false;

function setHeld(on: boolean) {
  held = on;
  for (const control of guarded) {
    if (on) control.setAttribute("aria-disabled", "true");
    else control.removeAttribute("aria-disabled");
  }
}

function hold() {
  clearTimeout(holdTimer);
  if ((userOpened && !away) || timing.holdMs <= 0) return setHeld(false);
  setHeld(true);
  holdTimer = setTimeout(() => setHeld(false), timing.holdMs);
}

/** Runs an acting control's action, unless the hold is on. */
function act(control: EventTarget) {
  if (held) return;
  actions.get(control)?.();
}

// Coming back to the window starts the wait again (the user may be typing in another window).
window.addEventListener("focus", hold);
window.addEventListener("blur", () => void (away = true));
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") hold();
  else away = true;
});

const plainKey = (e: KeyboardEvent) => !(e.isComposing || e.ctrlKey || e.altKey || e.metaKey || e.shiftKey);

// Enter and Space on an acting control: only a fresh key press (never one held down) and never during the hold.
// The browser's own activation is always stopped, so it cannot act past these rules.
document.addEventListener("keydown", (e) => {
  if (e.key !== "Enter" && e.key !== " ") return;
  if (!e.target || !actions.has(e.target)) return;
  e.preventDefault();
  if (e.repeat || !plainKey(e)) return;
  act(e.target);
});
document.addEventListener("keyup", (e) => {
  if ((e.key === "Enter" || e.key === " ") && e.target && actions.has(e.target)) e.preventDefault();
});

// Up and Down move between the rows (a picker's logins, generated password and passkeys). Moving does not act,
// so it works during the hold; Enter on the row still waits for the hold to end.
document.addEventListener("keydown", (e) => {
  if ((e.key !== "ArrowDown" && e.key !== "ArrowUp") || !plainKey(e)) return;
  const rows = [...main.querySelectorAll<HTMLButtonElement>("button.row")];
  if (!rows.length) return;
  e.preventDefault();
  const at = rows.indexOf(document.activeElement as HTMLButtonElement);
  const next = e.key === "ArrowDown" ? (at < 0 ? 0 : Math.min(at + 1, rows.length - 1)) : at < 0 ? rows.length - 1 : Math.max(at - 1, 0);
  rows[next]!.focus();
});

// ---- closing without choosing: Esc, and a click outside for the two decisions that can be taken back ----

/** The current view's way out (Cancel, Close or Not now); undefined for a view that only reports. */
let dismiss: (() => void) | undefined;
/** Set once the way out has been taken, so a second key press or a second blur does not send a second answer. */
let dismissed = false;
/** The decision this window is for (known after `decisionInfo`). */
let decision: DecisionInfo["decision"] | undefined;
/** True while an answer is carried out or the app's own prompt (Unlock) is on screen: the app takes focus then. */
let working = false;
/** A window that Wayland opened without focus must not be closed by the focus it never had. */
let hadFocus = document.hasFocus();
let outsideTimer: ReturnType<typeof setTimeout> | undefined;

function takeWayOut() {
  if (dismissed || !dismiss) return;
  dismissed = true;
  dismiss();
}

// Esc does what the Cancel button does, even during the hold (Cancel is safe). Not a key held down, and not
// the Esc that closes an input method's composition.
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || e.repeat || e.isComposing || e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
  e.preventDefault();
  takeWayOut();
});

/**
 * A click outside takes focus away. Only for choosing a login and, in a window, for saving one: a passkey
 * window is the answer to a site's own WebAuthn call, which a stray click on the page must not silently fail.
 * In the toolbar panel the browser itself closes the panel on a click outside, and the background decides
 * what that means: a save offer then waits (badge on the toolbar icon), it is not "Not now".
 */
const outsideCloses = () => (decision === "chooseLogin" || (!panel && decision === "saveLogin")) && hadFocus && !working && !!dismiss;
window.addEventListener("focus", () => {
  hadFocus = true;
  clearTimeout(outsideTimer);
});
window.addEventListener("blur", () => {
  clearTimeout(outsideTimer);
  if (!outsideCloses()) return;
  outsideTimer = setTimeout(() => {
    if (outsideCloses() && !document.hasFocus()) takeWayOut();
  }, timing.outsideMs);
});

function show(lines: string[], buttons: Button[] = [], rows: Row[] = []) {
  guarded = [];
  const nodes: Node[] = lines.map((line, i) => el("p", i === 0 ? "lead" : "line muted", line));
  if (rows.length) {
    const list = el("div", "rows");
    for (const r of rows) {
      const button = el("button", "row", r.label);
      button.type = "button";
      if (r.sub) button.append(el("span", "sub", r.sub));
      actions.set(button, r.onSelect);
      button.addEventListener("click", () => act(button));
      guarded.push(button);
      list.append(button);
    }
    nodes.push(list);
  }
  const bar = el("div", "buttons");
  for (const b of buttons) {
    const button = el("button", `${b.primary ? "primary" : ""}${b.safe ? " safe" : ""}`.trim(), b.label);
    button.type = "button";
    if (b.safe) button.addEventListener("click", b.onClick);
    else {
      actions.set(button, b.onClick);
      button.addEventListener("click", () => act(button));
      guarded.push(button);
    }
    bar.append(button);
  }
  nodes.push(bar);
  dismiss = buttons.find((b) => b.dismiss)?.onClick;
  if (dismiss) nodes.push(el("p", "hint", "Esc to close"));
  main.replaceChildren(...nodes);
  hold();
  // Nothing that acts is focused: a key the user was already pressing must not choose for them.
  (main.querySelector("button.safe") as HTMLElement | null)?.focus();
}

/** While LocalPass works: the app may ask the user to confirm (Windows Hello on Windows, the master password elsewhere). */
let confirmLine = "Confirm in the LocalPass app if it asks.";
void userCheckName()
  .then((name) => (confirmLine = `Confirm with ${name} if it asks.`))
  .catch(() => {});

const cancel: Button = { label: "Cancel", safe: true, dismiss: true, onClick: () => void answer({ action: "cancel" }).catch(() => window.close()) };
const close: Button = { label: "Close", safe: true, dismiss: true, onClick: () => cancel.onClick() };
const another: Button = { label: "Use another method", safe: true, onClick: () => void answer({ action: "fallback" }).catch(() => window.close()) };

/** Sends the answer; the background closes this window when it worked. */
async function carryOut(a: DecisionAnswer, failed: string, extra: Button[]) {
  show(["Waiting for LocalPass…", confirmLine], [cancel]);
  working = true;
  try {
    const done = await answer(a);
    if (done.message) show([done.message]);
  } catch (e) {
    // The window stays open to say what happened; closing it tells the page the same.
    paused = false;
    working = false;
    if (code(e) === "cancelled" || code(e) === "notFound") return window.close();
    show([code(e) === "denied" ? "Nothing was changed." : failed, text(e)], [...extra, close]);
  }
}

// ---- the four decisions ----

/**
 * The normal toolbar popup, in place of a panel whose request the background no longer has (never a dead panel).
 * This panel may have used up the background's count of its own `openPopup` when it attached, so the popup could not
 * tell it from one the user opened: `from=panel` makes it stay held and focus no Fill, whatever the background says.
 */
function toPopup() {
  location.replace(chrome.runtime.getURL("popup.html?from=panel"));
}

/** "The last try did not work": a failure while the panel was closed, shown when it is shown again. */
const failedLines = (info: DecisionInfo) => (info.failed ? [`The last try did not work: ${text(info.failed)}`] : []);

function unlockButton(again: () => void): Button {
  return {
    label: "Unlock",
    primary: true,
    onClick: () => {
      working = true;
      // The background follows the Unlock: if the app's prompt closes this panel, it opens it again afterwards.
      void ask({ kind: "decisionUnlock", id })
        .catch(() => undefined)
        .then(() => {
          working = false;
          again();
        });
    },
  };
}

function createPasskey(info: Extract<DecisionInfo, { decision: "passkeyCreate" }>) {
  show([...createDialogLines(info.request, info.host), ...failedLines(info)], [
    { label: "Create passkey", primary: true, onClick: () => void carryOut({ action: "create" }, "Could not create the passkey.", [another]) },
    another,
    cancel,
  ]);
}

async function choosePasskey(info: Extract<DecisionInfo, { decision: "passkeyGet" }>) {
  show(["Looking for your passkeys…"], [cancel]);
  let found: PasskeyCandidate[];
  try {
    found = await ask<PasskeyCandidate[]>({ kind: "decisionCandidates", id });
  } catch (e) {
    if (code(e) === "locked") return show(["LocalPass is locked.", "Unlock it to use your passkeys."], [unlockButton(() => void choosePasskey(info)), another]);
    if (UNAVAILABLE.has(code(e))) return void answer({ action: "fallback" }).catch(() => window.close());
    return show(["Could not look for passkeys.", text(e)], [another, close]);
  }
  if (!found.length) return void answer({ action: "fallback" }).catch(() => window.close());
  show(
    [displayText(info.rpId || info.host, 253), "Choose a passkey to sign in with:", ...failedLines(info)],
    [another, cancel],
    found.map((c) => ({ ...describePasskey(c), onSelect: () => void carryOut({ action: "assert", itemId: c.itemId }, "Could not sign in.", [another]) })),
  );
}

function saveLogin(info: Extract<DecisionInfo, { decision: "saveLogin" }>) {
  const offer = saveOffer(!!info.update);
  show(
    [offer.question, `${displayText(info.username, 60) || "(no username)"} on ${displayText(info.host, 80)}`, ...failedLines(info)],
    [
      { label: offer.button, primary: true, onClick: () => void carryOut({ action: "save" }, offer.failed, []) },
      { label: "Not now", safe: true, dismiss: true, onClick: cancel.onClick },
      { label: "Never for this site", onClick: () => void answer({ action: "never" }).catch(() => window.close()) },
    ],
  );
}

const WHY: Record<string, string> = {
  noVault: "Create or join a vault in the LocalPass app first.",
  unpaired: "Click the LocalPass icon in the browser toolbar and choose Connect.",
  denied: "Click the LocalPass icon in the browser toolbar and choose Connect.",
  untrusted: "Something answered that could not prove it is LocalPass. Nothing was shared.",
  tooMany: "LocalPass is busy right now. Try again in a moment.",
  tooLarge: "LocalPass could not send that answer (too much data).",
};

async function chooseLogin(info: Extract<DecisionInfo, { decision: "chooseLogin" }>) {
  show(["Looking for your logins…"], [cancel]);
  let reply: FindLoginsReply;
  try {
    reply = await ask<FindLoginsReply>({ kind: "decisionLogins", id });
  } catch (e) {
    reply = { status: code(e), logins: [] };
  }
  if (reply.status === "locked") return show(["LocalPass is locked.", "Unlock it to see your logins."], [unlockButton(() => void chooseLogin(info)), close]);
  if (reply.status !== "ok") return show([WHY[reply.status] ?? "Can't reach LocalPass. Check that the app is running, then try again."], [close]);
  // The page's pending passkey sign-in (Chrome): its passkeys first. None, or locked: no rows (the logins tell why).
  let found: PasskeyCandidate[] = [];
  if (info.passkeys) found = await ask<PasskeyCandidate[]>({ kind: "decisionCandidates", id }).catch(() => []);
  const rows: Row[] = [
    ...found.map((c) => {
      const text = describePasskey(c);
      return { label: `Passkey: ${text.label}`, sub: text.sub, onSelect: () => void carryOut({ action: "assert", itemId: c.itemId }, "Could not sign in.", []) };
    }),
    ...reply.logins.map((l) => ({
      label: displayText(l.title, 80) || displayText(l.username, 80),
      sub: displayText(l.username, 80) || "(no username)",
      onSelect: () => void carryOut({ action: "fill", itemId: l.id }, "Could not fill the login.", []),
    })),
  ];
  // A form that asks for a new password (sign-up, change password): the page's hint, so at worst a random password is offered.
  if (info.newPassword) {
    rows.push({ label: "Generate a strong password", sub: "Fills the new password boxes", onSelect: () => void carryOut({ action: "generate" }, "Could not make a password.", []) });
  }
  const host = displayText(info.host, 80);
  if (!rows.length) return show([`No saved logins for ${host} yet.`], [close]);
  show([found.length || reply.logins.length ? `Fill a login for ${host}` : `No saved logins for ${host} yet.`], [cancel], rows);
}

/**
 * Firefox suspends the background (an event page) after about 30 seconds without events, and an open
 * port does not stop that (measured, 6.6b); Chrome stops an idle service worker after 30 seconds without
 * events. The decision would be forgotten while the user is still reading. A message every few seconds
 * keeps it awake, and also notices when it was forgotten anyway.
 */
function keepAlive() {
  const timer = setInterval(() => {
    if (paused) return;
    ask<DecisionInfo>({ kind: "decisionInfo", id }).catch((e) => {
      if (code(e) !== "notFound") return;
      clearInterval(timer);
      if (panel) return toPopup();
      show(["This request is no longer open.", "LocalPass forgot it. Try again from the page."], [{ label: "Close", safe: true, dismiss: true, onClick: () => window.close() }]);
    });
  }, timing.keepAliveMs);
}

async function start() {
  // Closing the window ends the port, which the background takes as "no".
  const port = chrome.runtime.connect({ name: `decide:${id}` });
  if (panel) port.onDisconnect.addListener(() => window.close());
  // Only the background can send on this port: it says so when the user opened this panel (no hold then).
  if (panel)
    port.onMessage?.addListener((note: PanelNote) => {
      if (note?.userOpened !== true || userOpened) return;
      userOpened = true;
      hold();
    });
  keepAlive();
  let info: DecisionInfo;
  try {
    info = await ask<DecisionInfo>({ kind: "decisionInfo", id });
  } catch (e) {
    // A panel the toolbar still showed for a request that is gone (the background restarted): the normal popup.
    if (panel && code(e) === "notFound") return toPopup();
    return show(["This request is no longer open.", text(e)], [{ label: "Close", safe: true, dismiss: true, onClick: () => window.close() }]);
  }
  siteLabel.textContent = displayText(info.host, 80);
  decision = info.decision;
  render(info);
}

/**
 * Shown again while the background still carries out an answer given in an earlier showing (the app's prompt
 * took the focus and closed the panel): wait for it, then show the decision (with what went wrong, if it failed).
 * When it worked the background ends the decision, and this panel closes with its port.
 */
function waitForWork() {
  show(["Waiting for LocalPass…", confirmLine], [cancel]);
  working = true;
  const poll = setInterval(() => {
    ask<DecisionInfo>({ kind: "decisionInfo", id }).then(
      (next) => {
        if (next.working) return;
        clearInterval(poll);
        working = false;
        render(next);
      },
      () => clearInterval(poll),
    );
  }, timing.pollMs);
}

function render(info: DecisionInfo) {
  if (info.working) return waitForWork();
  switch (info.decision) {
    case "passkeyCreate":
      return createPasskey(info);
    case "passkeyGet":
      return choosePasskey(info);
    case "saveLogin":
      return saveLogin(info);
    case "chooseLogin":
      return chooseLogin(info);
  }
}

void start();
