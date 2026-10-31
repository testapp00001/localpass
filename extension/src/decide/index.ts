// The decision window (Firefox only). The background opened it for one request of one page and
// tells it what the request is (`decisionInfo`); the user decides here, and the background carries
// the answer out and hands any secret to the page's content script, never to this window. Everything
// the page supplied is shown only as text, through `displayText`, after the site the browser reported.

import { displayText } from "../shared/limits";
import type { DecisionAnswer, DecisionDone, DecisionInfo, FindLoginsReply, Reply, ToBackground } from "../shared/messages";
import { BrokerError, type PasskeyCandidate } from "../shared/protocol";
import { createDialogLines, describePasskey } from "../shared/shown";
import { timing } from "./timing";

const id = new URLSearchParams(location.search).get("id") ?? "";
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
  return ask<DecisionDone>({ kind: "decisionAnswer", id, answer: a });
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
  onClick: () => void;
}
interface Row {
  label: string;
  sub?: string;
  onSelect: () => void;
}

/** The controls that act for the user (everything but the safe buttons): disabled for `timing.holdMs` after each view and each focus. */
let guarded: HTMLButtonElement[] = [];
let holdTimer: ReturnType<typeof setTimeout> | undefined;

function hold() {
  clearTimeout(holdTimer);
  for (const control of guarded) control.disabled = true;
  const release = () => {
    for (const control of guarded) control.disabled = false;
  };
  if (timing.holdMs <= 0) release();
  else holdTimer = setTimeout(release, timing.holdMs);
}

// Coming back to the window starts the wait again (the user may be typing in another window).
window.addEventListener("focus", hold);
document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && hold());

function show(lines: string[], buttons: Button[] = [], rows: Row[] = []) {
  guarded = [];
  const nodes: Node[] = lines.map((line, i) => el("p", i === 0 ? "lead" : "line muted", line));
  if (rows.length) {
    const list = el("div", "rows");
    for (const r of rows) {
      const button = el("button", "row", r.label);
      button.type = "button";
      if (r.sub) button.append(el("span", "sub", r.sub));
      button.addEventListener("click", r.onSelect);
      guarded.push(button);
      list.append(button);
    }
    nodes.push(list);
  }
  const bar = el("div", "buttons");
  for (const b of buttons) {
    const button = el("button", `${b.primary ? "primary" : ""}${b.safe ? " safe" : ""}`.trim(), b.label);
    button.type = "button";
    button.addEventListener("click", b.onClick);
    if (!b.safe) guarded.push(button);
    bar.append(button);
  }
  nodes.push(bar);
  main.replaceChildren(...nodes);
  hold();
  // Nothing that acts is focused: a key the user was already pressing must not choose for them.
  (main.querySelector("button.safe") as HTMLElement | null)?.focus();
}

const cancel: Button = { label: "Cancel", safe: true, onClick: () => void answer({ action: "cancel" }).catch(() => window.close()) };
const close: Button = { label: "Close", safe: true, onClick: () => cancel.onClick() };
const another: Button = { label: "Use another method", safe: true, onClick: () => void answer({ action: "fallback" }).catch(() => window.close()) };

/** Sends the answer; the background closes this window when it worked. */
async function carryOut(a: DecisionAnswer, failed: string, extra: Button[]) {
  show(["Waiting for LocalPass…", "Confirm in the LocalPass app if it asks."], [cancel]);
  try {
    const done = await answer(a);
    if (done.message) show([done.message]);
  } catch (e) {
    // The window stays open to say what happened; closing it tells the page the same.
    paused = false;
    if (code(e) === "cancelled" || code(e) === "notFound") return window.close();
    show([code(e) === "denied" ? "Nothing was changed." : failed, text(e)], [...extra, close]);
  }
}

// ---- the four decisions ----

function unlockButton(again: () => void): Button {
  return {
    label: "Unlock",
    primary: true,
    onClick: () => void ask({ kind: "unlock" }).then(again, () => again()),
  };
}

function createPasskey(info: Extract<DecisionInfo, { decision: "passkeyCreate" }>) {
  show(createDialogLines(info.request, info.host), [
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
    [displayText(info.rpId || info.host, 253), "Choose a passkey to sign in with:"],
    [another, cancel],
    found.map((c) => ({ ...describePasskey(c), onSelect: () => void carryOut({ action: "assert", itemId: c.itemId }, "Could not sign in.", [another]) })),
  );
}

function saveLogin(info: Extract<DecisionInfo, { decision: "saveLogin" }>) {
  show(
    ["Save this login to LocalPass?", `${displayText(info.username, 60) || "(no username)"} on ${displayText(info.host, 80)}`],
    [
      { label: "Save", primary: true, onClick: () => void carryOut({ action: "save" }, "Could not save the login.", []) },
      { label: "Not now", safe: true, onClick: cancel.onClick },
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
  if (!reply.logins.length) return show([`No saved logins for ${displayText(info.host, 80)} yet.`], [close]);
  show(
    [`Fill a login for ${displayText(info.host, 80)}`],
    [cancel],
    reply.logins.map((l) => ({
      label: displayText(l.title, 80) || displayText(l.username, 80),
      sub: displayText(l.username, 80) || "(no username)",
      onSelect: () => void carryOut({ action: "fill", itemId: l.id }, "Could not fill the login.", []),
    })),
  );
}

/**
 * Firefox suspends the background (an event page) after about 30 seconds without events, and an open
 * port does not stop that (measured, 6.6b): the decision would be forgotten while the user is still
 * reading. A message every few seconds keeps it awake, and also notices when it was forgotten anyway.
 */
function keepAlive() {
  const timer = setInterval(() => {
    if (paused) return;
    ask<DecisionInfo>({ kind: "decisionInfo", id }).catch((e) => {
      if (code(e) !== "notFound") return;
      clearInterval(timer);
      show(["This request is no longer open.", "LocalPass forgot it. Try again from the page."], [{ label: "Close", onClick: () => window.close() }]);
    });
  }, timing.keepAliveMs);
}

async function start() {
  // Closing the window ends the port, which the background takes as "no".
  chrome.runtime.connect({ name: `decide:${id}` });
  keepAlive();
  let info: DecisionInfo;
  try {
    info = await ask<DecisionInfo>({ kind: "decisionInfo", id });
  } catch (e) {
    return show(["This request is no longer open.", text(e)], [{ label: "Close", onClick: () => window.close() }]);
  }
  siteLabel.textContent = displayText(info.host, 80);
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
