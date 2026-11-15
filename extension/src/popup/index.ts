// The toolbar popup: connect to the app, unlock, and fill the page you are on.

import type { ItemSummary, PanelNote, PopupState, PrivacyState, Reply, ToBackground } from "../shared/messages";
import { POPUP_PORT } from "../shared/messages";
import { local } from "../background/storage";
import { timing } from "../decide/timing";
import { BrokerError, type LoginMatch } from "../shared/protocol";
import { browserName, userCheckName } from "../shared/platform";

/** How the app confirms it is you: Windows Hello on Windows, the master password elsewhere. Known before the first render. */
let CONFIRM = "";
const confirmKnown = userCheckName().then((name) => {
  CONFIRM = name;
});

const main = document.getElementById("main") as HTMLElement;
const site = document.getElementById("site") as HTMLElement;
const privacyBox = document.getElementById("privacy") as HTMLInputElement;
const privacyNote = document.getElementById("privacy-note") as HTMLElement;
const passkeysBox = document.getElementById("passkeys") as HTMLInputElement;
// Focusable without acting: where the focus goes in a popup the user did not open.
main.tabIndex = -1;

// ---- who opened the popup, and the hold ----

/**
 * The background's own `action.openPopup` can show this popup instead of a decision panel (the browser reset the
 * tab's popup in between, or a panel whose request was gone replaced itself with this page), at a moment a page
 * chose. So, as in the decision panel, the controls that act (buttons, the two switches) do nothing for
 * `timing.holdMs` after each view and each return to the window, and no Fill has the focus. Only the background's
 * note on this popup's own port, sent when no own `openPopup` was waiting in this window, says the user opened it
 * (toolbar icon or shortcut): then nothing is held and the first Fill for the tab's site has the focus, so the
 * shortcut and then Enter fills. Nothing a page or content script sends can say so. After the popup lost the focus
 * (or was hidden) once, coming back holds again.
 */
let userOpened = false;
/**
 * A decision panel whose request was gone turned into this popup (`toPopup` in decide/index.ts). That panel may have
 * taken the background's count of its own `openPopup`, so the background would wrongly say the user opened this
 * popup: it never asks, and is always held like a popup the background showed. Only extension pages choose this URL.
 */
const fromPanel = new URLSearchParams(location.search).get("from") === "panel";
let away = false;
let held = false;
let holdTimer: ReturnType<typeof setTimeout> | undefined;
/** The first Fill of a login for the tab's own site in the current view (focused only in a popup the user opened). */
let firstFill: HTMLButtonElement | null = null;

const ACTING = "button, input[type=checkbox]";
const acting = (target: EventTarget | null) => target instanceof Element && !!target.closest(ACTING);

function setHeld(on: boolean) {
  held = on;
  for (const control of document.querySelectorAll(ACTING)) {
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

/** Where the focus goes with a new view: the first own-site Fill when the user opened the popup, else nothing that acts. */
function placeFocus() {
  if (firstFill && userOpened && !away) return firstFill.focus();
  if (acting(document.activeElement)) main.focus();
}

// While held, no click (a mouse, or the browser's own activation by Enter or Space) reaches an acting control.
document.addEventListener(
  "click",
  (e) => {
    if (!held || !acting(e.target)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
  },
  true,
);
// Enter and Space never act while held, nor when the key is held down (a key repeat from typing elsewhere).
document.addEventListener(
  "keydown",
  (e) => {
    if ((e.key === "Enter" || e.key === " ") && acting(e.target) && (held || e.repeat)) e.preventDefault();
  },
  true,
);
document.addEventListener(
  "keyup",
  (e) => {
    if (e.key === " " && acting(e.target) && held) e.preventDefault();
  },
  true,
);
window.addEventListener("focus", hold);
window.addEventListener("blur", () => void (away = true));
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") hold();
  else away = true;
});

/** Asks the background who opened this popup, on this popup's own port. Without an answer the popup stays held. */
async function listen() {
  if (fromPanel) return;
  const win = await chrome.windows.getCurrent();
  if (win?.id === undefined) return;
  const port = chrome.runtime.connect({ name: `${POPUP_PORT}${win.id}` });
  port.onMessage.addListener((note: PanelNote) => {
    if (note?.userOpened !== true || userOpened) return;
    userOpened = true;
    hold();
    // The view may already be shown: move the focus to its first Fill unless the user moved it.
    if (!acting(document.activeElement)) placeFocus();
  });
}

async function ask<T>(message: ToBackground): Promise<T> {
  const reply = (await chrome.runtime.sendMessage(message)) as Reply<T>;
  if (!reply.ok) throw new BrokerError(reply.code, reply.message);
  return reply.data;
}

function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> & { class?: string } = {}, ...children: (Node | string)[]): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  const { class: className, ...rest } = props;
  if (className) node.className = className;
  Object.assign(node, rest);
  node.append(...children);
  return node;
}

function button(label: string, onClick: () => void, primary = false): HTMLButtonElement {
  const b = h("button", { type: "button", class: primary ? "primary" : "" }, label);
  if (held) b.setAttribute("aria-disabled", "true");
  b.addEventListener("click", async () => {
    b.disabled = true;
    try {
      await onClick();
    } finally {
      b.disabled = false;
    }
  });
  return b;
}

function show(...nodes: Node[]) {
  firstFill = null;
  main.replaceChildren(...nodes);
  hold();
  placeFocus();
}

function message(title: string, text: string, ...actions: Node[]) {
  show(h("h2", {}, title), h("p", { class: "muted" }, text), ...actions);
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ---- lists ----

/**
 * One login and its Fill. For a login saved for another site (`force`), Fill first opens a confirmation step in the
 * row (what will happen, "Fill anyway" and Back, which gets the focus); the app then asks for its own user check
 * before it fills. Errors are shown in the row, as text.
 */
function loginRow(login: { id: string; title: string; username: string }, state: PopupState, force: boolean): HTMLElement {
  const status = h("span", { class: "muted" });
  const say = (text: string, error = false) => {
    status.className = error ? "error" : "muted";
    status.textContent = text;
  };
  const fill = async () => {
    say("");
    try {
      await ask({ kind: "fillTab", tabId: state.tab!.id, itemId: login.id, force });
      window.close();
    } catch (e) {
      // Saying no to Windows Hello is an answer, not a failure: say what happened, quietly.
      if (e instanceof BrokerError && e.code === "denied") say("Not filled: it was not confirmed.");
      else say(errorText(e), true);
    }
  };
  const who = h("div", { class: "who" }, h("b", {}, login.title), h("span", {}, login.username || "(no username)"), status);
  const row = h("li", {});
  const question = h("div", { class: "confirm" });
  const confirmStep = () => {
    say("");
    const back = button("Back", () => {
      question.remove();
      row.append(fillButton);
      fillButton.focus();
    });
    question.replaceChildren(
      h("p", { class: "small" }, `Saved for a different website than ${state.tab?.host ?? "this page"}. LocalPass will ask for ${CONFIRM} before it fills it here.`),
      h("div", { class: "actions" }, button("Fill anyway", fill, true), back),
    );
    fillButton.remove();
    row.append(question);
    back.focus();
  };
  const fillButton = button("Fill", force ? confirmStep : fill, !force);
  row.append(who, fillButton);
  return row;
}

function summaryRow(item: ItemSummary, state: PopupState): HTMLElement {
  return loginRow({ id: item.id, title: item.title, username: item.subtitle }, state, true);
}

/** The most logins the app sends in one list (the popup says so when it got that many). */
const LIST_LIMIT = 200;

/** Whether a login matches what was typed in the filter box (title, user name, address). */
export function matchesFilter(item: ItemSummary, filter: string): boolean {
  const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
  const text = `${item.title} ${item.subtitle} ${item.url ?? ""}`.toLowerCase();
  return words.every((w) => text.includes(w));
}

/**
 * Logins saved for other sites. Getting the list is one explicit action that makes the app ask
 * for Windows Hello (once); typing only filters that list here and asks nothing.
 */
function searchSection(state: PopupState): HTMLElement {
  const box = h("div", {});
  const note = h("p", { class: "muted" });
  const results = h("ul", { class: "list" });
  const input = h("input", { type: "search", placeholder: "Filter these logins" });

  const showList = (items: ItemSummary[]) => {
    const logins = items.filter((i) => i.kind === "login" && !i.trashed);
    const draw = () => {
      const rows = logins.filter((i) => matchesFilter(i, input.value)).map((i) => summaryRow(i, state));
      results.replaceChildren(...(rows.length ? rows : [h("li", { class: "muted" }, "Nothing found")]));
    };
    input.oninput = draw;
    note.textContent = items.length >= LIST_LIMIT ? `Showing the first ${LIST_LIMIT} logins. Filter to narrow them down.` : `${logins.length} logins in your vault.`;
    box.replaceChildren(note, input, results);
    draw();
  };

  const load = button("Show all my logins", async () => {
    note.className = "muted";
    note.textContent = `Waiting for ${CONFIRM} in LocalPass…`;
    try {
      showList(await ask<ItemSummary[]>({ kind: "loadLogins" }));
    } catch (e) {
      // Saying no to Windows Hello is an answer, not a failure.
      if (e instanceof BrokerError && e.code === "denied") note.textContent = "Not shown: it was not confirmed in LocalPass.";
      else {
        note.className = "error";
        note.textContent = errorText(e);
      }
    }
  });
  note.textContent = `To fill a login saved for another site, LocalPass asks for ${CONFIRM} once when you list your logins, and again before it fills one.`;
  box.append(note, load, results);
  // The popup closes while Windows Hello has the focus: the list asked for a moment ago is kept.
  void ask<ItemSummary[] | null>({ kind: "cachedLogins" })
    .then((items) => items && showList(items))
    .catch(() => {});
  return box;
}

// ---- states ----

function render(state: PopupState) {
  site.textContent = state.tab?.host ?? "";
  const { phase, code, detail } = state.broker;
  const retry = button("Try again", () => refresh(), true);
  switch (phase) {
    case "ready":
      return renderReady(state);
    case "unpaired":
      return message(
        state.broker.forgotten ? "Connect again" : "Connect to LocalPass",
        state.broker.forgotten
          ? "LocalPass no longer knows this browser (it was removed in the app, or the app was reset). Pair it again to keep using it here. You will confirm a code in the app."
          : "Link this browser to the LocalPass app on this computer. You will confirm a code in the app.",
        button(state.broker.forgotten ? "Connect again" : "Connect", async () => {
          await ask({ kind: "connect" });
          await refresh();
        }, true),
      );
    case "pairing":
      return show(
        h("h2", {}, "Confirm in LocalPass"),
        h("p", { class: "muted" }, "Open the LocalPass app and check that it shows this code, then choose Connect."),
        h("div", { class: "code" }, code ?? "······"),
      );
    case "denied":
      return message("Not connected", "The connection was declined in LocalPass.", button("Connect again", async () => {
        await ask({ kind: "connect" });
        await refresh();
      }, true));
    case "appNotRunning":
      return message(
        "LocalPass isn't available",
        "The LocalPass app is not running, or the browser connection is not set up. In the app, open Settings → Browser extension and choose Set up.",
        retry,
      );
    case "untrusted":
      return message("Not connected", detail ?? "Something answered that could not prove it is LocalPass.", retry);
    default:
      return message("Not connected", detail ?? "Waiting for LocalPass…", retry);
  }
}

function renderReady(state: PopupState) {
  if (state.vault === "noVault") {
    return message("No vault yet", "Create or join a vault in the LocalPass app first.");
  }
  if (state.vault === "locked") {
    const failed = h("p", { class: "error" });
    return message(
      "LocalPass is locked",
      "Unlock it to fill logins.",
      button("Unlock", async () => {
        failed.textContent = "";
        try {
          await ask({ kind: "unlock" });
        } catch (e) {
          // Shown here, under the button, for another try (a change of state redraws the popup anyway).
          failed.textContent = errorText(e);
          return;
        }
        await refresh();
      }, true),
      failed,
    );
  }
  const nodes: Node[] = [];
  if (!state.tab?.origin) {
    nodes.push(h("p", { class: "muted" }, "This page can't be filled."));
  } else if (state.logins.length) {
    const rows = state.logins.map((l: LoginMatch) => loginRow(l, state, false));
    nodes.push(h("h2", {}, `Logins for ${state.tab.host}`), h("ul", { class: "list" }, ...rows));
    show(...nodes, searchSection(state));
    // The first Fill for this site: focused when the user opened the popup (`placeFocus`), so the shortcut then Enter fills.
    firstFill = rows[0]?.querySelector("button") ?? null;
    return placeFocus();
  } else {
    nodes.push(h("p", { class: "muted" }, `No saved logins for ${state.tab.host}. Sign in once and LocalPass will offer to save it.`));
  }
  if (state.tab?.origin) nodes.push(searchSection(state));
  show(...nodes);
}

async function refresh() {
  try {
    const state = await ask<PopupState>({ kind: "state" });
    await confirmKnown;
    render(state);
  } catch (e) {
    message("Something went wrong", errorText(e));
  }
}

// ---- the browser's own password manager ----

async function loadPrivacy() {
  try {
    const state = await ask<PrivacyState>({ kind: "getPrivacy" });
    privacyBox.checked = state.disabled;
    const locked = state.control === "not_controllable" || state.control === "controlled_by_other_extensions";
    privacyBox.disabled = locked;
    const browser = browserName();
    privacyNote.className = "muted small";
    privacyNote.textContent = locked ? "Your administrator or another extension controls this setting." : state.disabled ? `${browser} will no longer offer to save passwords.` : `${browser} may still offer to save passwords. Turn this on to use only LocalPass.`;
  } catch {
    privacyBox.disabled = true;
  }
}

privacyBox.addEventListener("change", async () => {
  let failed = "";
  try {
    await ask({ kind: "setPrivacy", disableBrowserManager: privacyBox.checked });
  } catch (e) {
    failed = errorText(e);
  }
  await loadPrivacy();
  // Shown in the setting's own note, as text.
  if (failed) {
    privacyNote.className = "error small";
    privacyNote.textContent = failed;
  }
});

// ---- passkeys ----

async function loadPasskeys() {
  // Through local(): in Firefox this setting lives in the extension's IndexedDB, not in storage.local.
  const stored = await (await local()).get("passkeysEnabled");
  passkeysBox.checked = stored.passkeysEnabled !== false;
}

passkeysBox.addEventListener("change", () => void local().then((area) => area.set({ passkeysEnabled: passkeysBox.checked })));

chrome.runtime.onMessage.addListener((m: { kind?: string }) => {
  if (m.kind === "brokerState") void refresh();
});

hold();
void listen().catch(() => {});
void refresh();
void loadPrivacy();
void loadPasskeys();
