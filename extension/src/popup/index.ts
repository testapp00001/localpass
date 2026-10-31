// The toolbar popup: connect to the app, unlock, and fill the page you are on.

import type { ItemSummary, PopupState, PrivacyState, Reply, ToBackground } from "../shared/messages";
import { local } from "../background/storage";
import { BrokerError, type LoginMatch } from "../shared/protocol";

/** Set by `bun run build --firefox` (Linux): the app confirms with the master password there, not Windows Hello. */
declare const __FIREFOX__: boolean;
const CONFIRM = __FIREFOX__ ? "your master password" : "Windows Hello";

const main = document.getElementById("main") as HTMLElement;
const site = document.getElementById("site") as HTMLElement;
const privacyBox = document.getElementById("privacy") as HTMLInputElement;
const privacyNote = document.getElementById("privacy-note") as HTMLElement;
const passkeysBox = document.getElementById("passkeys") as HTMLInputElement;

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
  main.replaceChildren(...nodes);
}

function message(title: string, text: string, ...actions: Node[]) {
  show(h("h2", {}, title), h("p", { class: "muted" }, text), ...actions);
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ---- lists ----

function loginRow(login: { id: string; title: string; username: string }, state: PopupState, force: boolean): HTMLElement {
  const status = h("span", { class: "muted" });
  const fillIt = async () => {
    status.textContent = "";
    if (force && !confirm(`"${login.title}" was saved for a different website than ${state.tab?.host ?? "this page"}.\n\nLocalPass will ask for ${CONFIRM} before it fills this login here. Continue?`)) return;
    try {
      await ask({ kind: "fillTab", tabId: state.tab!.id, itemId: login.id, force });
      window.close();
    } catch (e) {
      // Saying no to Windows Hello is an answer, not a failure: say what happened, quietly.
      if (e instanceof BrokerError && e.code === "denied") status.textContent = "Not filled: it was not confirmed.";
      else alert(errorText(e));
    }
  };
  return h("li", {}, h("div", { class: "who" }, h("b", {}, login.title), h("span", {}, login.username || "(no username)"), status), button("Fill", fillIt, !force));
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
    return message(
      "LocalPass is locked",
      "Unlock it to fill logins.",
      button("Unlock", async () => {
        try {
          await ask({ kind: "unlock" });
        } catch (e) {
          alert(errorText(e));
        }
        await refresh();
      }, true),
    );
  }
  const nodes: Node[] = [];
  if (!state.tab?.origin) {
    nodes.push(h("p", { class: "muted" }, "This page can't be filled."));
  } else if (state.logins.length) {
    nodes.push(h("h2", {}, `Logins for ${state.tab.host}`), h("ul", { class: "list" }, ...state.logins.map((l: LoginMatch) => loginRow(l, state, false))));
  } else {
    nodes.push(h("p", { class: "muted" }, `No saved logins for ${state.tab.host}. Sign in once and LocalPass will offer to save it.`));
  }
  if (state.tab?.origin) nodes.push(searchSection(state));
  show(...nodes);
}

async function refresh() {
  try {
    render(await ask<PopupState>({ kind: "state" }));
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
    privacyNote.textContent = locked ? "Your administrator or another extension controls this setting." : state.disabled ? "Chrome and Edge will no longer offer to save passwords." : "Chrome or Edge may still offer to save passwords. Turn this on to use only LocalPass.";
  } catch {
    privacyBox.disabled = true;
  }
}

privacyBox.addEventListener("change", async () => {
  try {
    await ask({ kind: "setPrivacy", disableBrowserManager: privacyBox.checked });
  } catch (e) {
    alert(errorText(e));
  }
  await loadPrivacy();
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

void refresh();
void loadPrivacy();
void loadPasskeys();
