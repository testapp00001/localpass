// src/shared/protocol.ts
class BrokerError extends Error {
  code;
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

// src/popup/index.ts
var main = document.getElementById("main");
var site = document.getElementById("site");
var privacyBox = document.getElementById("privacy");
var privacyNote = document.getElementById("privacy-note");
var passkeysBox = document.getElementById("passkeys");
async function ask(message) {
  const reply = await chrome.runtime.sendMessage(message);
  if (!reply.ok)
    throw new BrokerError(reply.code, reply.message);
  return reply.data;
}
function h(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  const { class: className, ...rest } = props;
  if (className)
    node.className = className;
  Object.assign(node, rest);
  node.append(...children);
  return node;
}
function button(label, onClick, primary = false) {
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
function show(...nodes) {
  main.replaceChildren(...nodes);
}
function message(title, text, ...actions) {
  show(h("h2", {}, title), h("p", { class: "muted" }, text), ...actions);
}
function errorText(e) {
  return e instanceof Error ? e.message : String(e);
}
function loginRow(login, state, force) {
  const fillIt = async () => {
    if (force && !confirm(`"${login.title}" was saved for a different website than ${state.tab?.host ?? "this page"}.

Fill it here anyway?`))
      return;
    try {
      await ask({ kind: "fillTab", tabId: state.tab.id, itemId: login.id, force });
      window.close();
    } catch (e) {
      alert(errorText(e));
    }
  };
  return h("li", {}, h("div", { class: "who" }, h("b", {}, login.title), h("span", {}, login.username || "(no username)")), button("Fill", fillIt, !force));
}
function summaryRow(item, state) {
  return loginRow({ id: item.id, title: item.title, username: item.subtitle }, state, true);
}
function searchSection(state) {
  const input = h("input", { type: "search", placeholder: "Search all items" });
  const results = h("ul", { class: "list" });
  let timer;
  input.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const query = input.value.trim();
      if (!query)
        return results.replaceChildren();
      try {
        const items = await ask({ kind: "search", query });
        const rows = items.filter((i) => i.kind === "login" && !i.trashed).map((i) => summaryRow(i, state));
        results.replaceChildren(...rows.length ? rows : [h("li", { class: "muted" }, "Nothing found")]);
      } catch (e) {
        results.replaceChildren(h("li", { class: "error" }, errorText(e)));
      }
    }, 200);
  });
  return h("div", {}, input, results);
}
function render(state) {
  site.textContent = state.tab?.host ?? "";
  const { phase, code, detail } = state.broker;
  const retry = button("Try again", () => refresh(), true);
  switch (phase) {
    case "ready":
      return renderReady(state);
    case "unpaired":
      return message("Connect to LocalPass", "Link this browser to the LocalPass app on this computer. You will confirm a code in the app.", button("Connect", async () => {
        await ask({ kind: "connect" });
        await refresh();
      }, true));
    case "pairing":
      return show(h("h2", {}, "Confirm in LocalPass"), h("p", { class: "muted" }, "Open the LocalPass app and check that it shows this code, then choose Connect."), h("div", { class: "code" }, code ?? "······"));
    case "denied":
      return message("Not connected", "The connection was declined in LocalPass.", button("Connect again", async () => {
        await ask({ kind: "connect" });
        await refresh();
      }, true));
    case "appNotRunning":
      return message("LocalPass isn't available", "The LocalPass app is not running, or the browser connection is not set up. In the app, open Settings → Browser extension and choose Set up.", retry);
    case "untrusted":
      return message("Not connected", detail ?? "Something answered that could not prove it is LocalPass.", retry);
    default:
      return message("Not connected", detail ?? "Waiting for LocalPass…", retry);
  }
}
function renderReady(state) {
  if (state.vault === "noVault") {
    return message("No vault yet", "Create or join a vault in the LocalPass app first.");
  }
  if (state.vault === "locked") {
    return message("LocalPass is locked", "Unlock it to fill logins.", button("Unlock", async () => {
      try {
        await ask({ kind: "unlock" });
      } catch (e) {
        alert(errorText(e));
      }
      await refresh();
    }, true));
  }
  const nodes = [];
  if (!state.tab?.origin) {
    nodes.push(h("p", { class: "muted" }, "This page can't be filled."));
  } else if (state.logins.length) {
    nodes.push(h("h2", {}, `Logins for ${state.tab.host}`), h("ul", { class: "list" }, ...state.logins.map((l) => loginRow(l, state, false))));
  } else {
    nodes.push(h("p", { class: "muted" }, `No saved logins for ${state.tab.host}. Sign in once and LocalPass will offer to save it.`));
  }
  if (state.tab?.origin)
    nodes.push(searchSection(state));
  show(...nodes);
}
async function refresh() {
  try {
    render(await ask({ kind: "state" }));
  } catch (e) {
    message("Something went wrong", errorText(e));
  }
}
async function loadPrivacy() {
  try {
    const state = await ask({ kind: "getPrivacy" });
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
async function loadPasskeys() {
  const stored = await chrome.storage.local.get("passkeysEnabled");
  passkeysBox.checked = stored.passkeysEnabled !== false;
}
passkeysBox.addEventListener("change", () => void chrome.storage.local.set({ passkeysEnabled: passkeysBox.checked }));
chrome.runtime.onMessage.addListener((m) => {
  if (m.kind === "brokerState")
    refresh();
});
refresh();
loadPrivacy();
loadPasskeys();
