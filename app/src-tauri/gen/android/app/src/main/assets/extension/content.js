(() => {
  // src/shared/protocol.ts
  class BrokerError extends Error {
    code;
    constructor(code, message) {
      super(message);
      this.code = code;
    }
  }

  // src/content/ask.ts
  async function ask(message) {
    const reply = await chrome.runtime.sendMessage(message);
    if (!reply.ok)
      throw new BrokerError(reply.code, reply.message);
    return reply.data;
  }

  // src/content/detect.ts
  var USERNAME_HINT = /user|login|e-?mail|account|identifier|\bid\b|phone|member/i;
  var SEARCH_HINT = /search|query|find|filter/i;
  var OTP_HINT = /otp|one.?time|2fa|two.?factor|totp|authenticator|verification|security.?code|passcode|\bcode\b|token/i;
  var NEW_PASSWORD_HINT = /new|confirm|repeat|retype|again/i;
  function attr(el, name) {
    return el.getAttribute(name) ?? "";
  }
  function hintText(el) {
    return [attr(el, "name"), el.id, attr(el, "placeholder"), attr(el, "aria-label"), attr(el, "autocomplete")].join(" ");
  }
  function autocompleteTokens(el) {
    return attr(el, "autocomplete").toLowerCase().split(/\s+/).filter(Boolean);
  }
  function isVisible(el, options = {}) {
    if (el.hidden)
      return false;
    const view = el.ownerDocument.defaultView;
    const style = view?.getComputedStyle(el);
    if (style && (style.display === "none" || style.visibility === "hidden"))
      return false;
    if (options.assumeVisible)
      return true;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }
  function isFillable(el, options = {}) {
    return !el.disabled && !el.readOnly && el.type !== "hidden" && isVisible(el, options);
  }
  function isTextual(el) {
    return ["text", "email", "tel", "url", "number", ""].includes(el.type.toLowerCase());
  }
  function isUsernameCandidate(el) {
    if (!isTextual(el))
      return false;
    const tokens = autocompleteTokens(el);
    if (tokens.includes("one-time-code"))
      return false;
    if (SEARCH_HINT.test(hintText(el)) && !tokens.includes("username"))
      return false;
    return true;
  }
  function isOtpField(el) {
    if (!["text", "tel", "number", "password", ""].includes(el.type.toLowerCase()))
      return false;
    if (autocompleteTokens(el).includes("one-time-code"))
      return true;
    const max = Number(attr(el, "maxlength"));
    const shortField = max >= 4 && max <= 10;
    return shortField && OTP_HINT.test(hintText(el)) && !USERNAME_HINT.test(attr(el, "name") + el.id);
  }
  function isNewPasswordField(el) {
    return autocompleteTokens(el).includes("new-password") || NEW_PASSWORD_HINT.test(attr(el, "name") + " " + el.id);
  }
  function before(a, b) {
    return !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
  }
  function containerOf(anchor) {
    if (anchor.form)
      return anchor.form;
    let node = anchor;
    for (let depth = 0;depth < 5 && node.parentElement; depth++) {
      node = node.parentElement;
      if (node.querySelectorAll("input").length > 1)
        return node;
    }
    return anchor.parentElement ?? anchor;
  }
  function pickUsername(root, password, options) {
    const candidates = [...root.querySelectorAll("input")].filter((el) => el !== password && isUsernameCandidate(el) && isFillable(el, options));
    const preceding = candidates.filter((el) => before(el, password));
    const pool = preceding.length ? preceding : candidates;
    if (!pool.length)
      return null;
    const score = (el) => {
      let s = 0;
      const tokens = autocompleteTokens(el);
      if (tokens.includes("username"))
        s += 6;
      if (tokens.includes("email"))
        s += 4;
      if (el.type === "email")
        s += 3;
      if (USERNAME_HINT.test(hintText(el)))
        s += 2;
      s += preceding.indexOf(el) + 1 > 0 ? (preceding.indexOf(el) + 1) / preceding.length : 0;
      return s;
    };
    return pool.reduce((best, el) => score(el) > score(best) ? el : best);
  }
  function findLoginForms(doc, options = {}) {
    const forms = [];
    const claimed = new Set;
    const passwords = [...doc.querySelectorAll('input[type="password"]')].filter((el) => isFillable(el, options));
    for (const password of passwords) {
      if (claimed.has(password))
        continue;
      const root = containerOf(password);
      const sameRoot = passwords.filter((p) => !claimed.has(p) && containerOf(p) === root);
      sameRoot.forEach((p) => claimed.add(p));
      const [first, ...rest] = sameRoot;
      if (!first)
        continue;
      const isNew = sameRoot.some(isNewPasswordField) || sameRoot.length >= 2;
      const main = isNew ? sameRoot.find(isNewPasswordField) ?? first : first;
      const otherBoxes = sameRoot.filter((p) => p !== main);
      forms.push({
        root,
        username: pickUsername(root, main, options),
        password: main,
        extraPasswords: isNew ? otherBoxes : rest,
        otp: findOtp(root, options),
        isNewPassword: isNew
      });
    }
    const identifiers = [...doc.querySelectorAll("input")].filter((el) => isTextual(el) && isFillable(el, options) && !claimed.has(el) && isIdentifierField(el));
    for (const el of identifiers) {
      const root = containerOf(el);
      if (forms.some((f) => f.root.contains(el)))
        continue;
      forms.push({ root, username: el, password: null, extraPasswords: [], otp: findOtp(root, options), isNewPassword: false });
    }
    for (const el of doc.querySelectorAll("input")) {
      if (!isOtpField(el) || !isFillable(el, options) || forms.some((f) => f.root.contains(el)))
        continue;
      forms.push({ root: containerOf(el), username: null, password: null, extraPasswords: [], otp: el, isNewPassword: false });
    }
    return forms;
  }
  function isIdentifierField(el) {
    const tokens = autocompleteTokens(el);
    return tokens.includes("username") || tokens.includes("email") && !tokens.includes("new-password");
  }
  function findOtp(root, options) {
    return [...root.querySelectorAll("input")].find((el) => isOtpField(el) && isFillable(el, options)) ?? null;
  }
  function setValue(el, value) {
    const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), "value")?.set ?? Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    el.dispatchEvent(new FocusEvent("focus"));
    if (setter)
      setter.call(el, value);
    else
      el.value = value;
    el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }

  // src/content/ui.ts
  var STYLE = `
  :host { all: initial; }
  * { box-sizing: border-box; font-family: system-ui, "Segoe UI", Roboto, sans-serif; }
  .badge { position: fixed; width: 22px; height: 22px; border: 0; border-radius: 6px; padding: 0; cursor: pointer;
    background: #4f46e5; color: #fff; font: 700 13px/22px system-ui, sans-serif; text-align: center; box-shadow: 0 1px 3px rgba(0,0,0,.35); }
  .badge.muted { background: #64748b; }
  .badge:hover { filter: brightness(1.15); }
  .panel { position: fixed; min-width: 240px; max-width: 340px; max-height: 360px; overflow: auto; background: #fff; color: #0f172a;
    border: 1px solid #cbd5e1; border-radius: 10px; box-shadow: 0 8px 28px rgba(15,23,42,.28); font-size: 13px; }
  @media (prefers-color-scheme: dark) { .panel { background: #0f172a; color: #e2e8f0; border-color: #334155; } .row:hover { background: #1e293b !important; } .muted-text { color: #94a3b8 !important; } }
  .title { padding: 8px 12px; font-weight: 600; font-size: 12px; color: #4f46e5; border-bottom: 1px solid rgba(100,116,139,.25); }
  .message { padding: 10px 12px; line-height: 1.4; }
  .muted-text { color: #64748b; font-size: 12px; }
  .row { display: block; width: 100%; text-align: left; border: 0; background: transparent; color: inherit; padding: 8px 12px; cursor: pointer; font-size: 13px; }
  .row:hover, .row:focus { background: #eef2ff; outline: none; }
  .row .sub { display: block; font-size: 12px; color: #64748b; }
  .bar { position: fixed; top: 12px; right: 12px; width: 340px; background: #fff; color: #0f172a; border: 1px solid #cbd5e1; border-radius: 12px;
    box-shadow: 0 10px 30px rgba(15,23,42,.3); padding: 14px; font-size: 13px; }
  @media (prefers-color-scheme: dark) { .bar { background: #0f172a; color: #e2e8f0; border-color: #334155; } }
  .bar b { color: #4f46e5; }
  .buttons { display: flex; gap: 8px; margin-top: 12px; flex-wrap: wrap; }
  .btn { border: 1px solid #cbd5e1; background: transparent; color: inherit; border-radius: 8px; padding: 6px 12px; cursor: pointer; font-size: 13px; }
  .btn.primary { background: #4f46e5; border-color: #4f46e5; color: #fff; }
  .backdrop { position: fixed; inset: 0; background: rgba(15,23,42,.45); display: flex; align-items: center; justify-content: center; }
  .sheet { width: min(380px, calc(100vw - 32px)); background: #fff; color: #0f172a; border-radius: 14px; box-shadow: 0 20px 50px rgba(15,23,42,.4);
    padding: 18px; font-size: 14px; max-height: calc(100vh - 32px); overflow: auto; }
  @media (prefers-color-scheme: dark) { .sheet { background: #0f172a; color: #e2e8f0; border: 1px solid #334155; } }
  .sheet h3 { margin: 0 0 6px; font-size: 16px; }
  .sheet p { margin: 0 0 8px; line-height: 1.45; }
  .sheet .rows { margin: 8px -6px; }
`;
  var root = null;
  function shadow() {
    if (root)
      return root;
    const host = document.createElement("div");
    host.style.cssText = "all: initial; position: fixed; top: 0; left: 0; width: 0; height: 0; z-index: 2147483647;";
    root = host.attachShadow({ mode: "closed" });
    const style = document.createElement("style");
    style.textContent = STYLE;
    root.append(style);
    document.documentElement.append(host);
    return root;
  }
  function el(tag, className, text) {
    const node = document.createElement(tag);
    node.className = className;
    if (text !== undefined)
      node.textContent = text;
    return node;
  }
  var badge = null;
  function showBadge(anchor, muted, onClick) {
    hideBadge();
    const button = el("button", muted ? "badge muted" : "badge", "L");
    button.type = "button";
    button.title = "LocalPass";
    button.addEventListener("mousedown", (e) => e.preventDefault());
    button.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      onClick();
    });
    shadow().append(button);
    const place = () => {
      if (!badge)
        return;
      if (!anchor.isConnected)
        return hideBadge();
      const r = anchor.getBoundingClientRect();
      button.style.left = `${Math.round(r.right - 28)}px`;
      button.style.top = `${Math.round(r.top + (r.height - 22) / 2)}px`;
      badge.frame = requestAnimationFrame(place);
    };
    badge = { button, anchor, frame: 0 };
    place();
  }
  function hideBadge() {
    if (!badge)
      return;
    cancelAnimationFrame(badge.frame);
    badge.button.remove();
    badge = null;
  }
  var panel = null;
  var dismiss = null;
  function hideMenu() {
    panel?.remove();
    panel = null;
    if (dismiss) {
      document.removeEventListener("mousedown", dismiss, true);
      document.removeEventListener("keydown", dismiss, true);
      window.removeEventListener("scroll", dismiss, true);
      dismiss = null;
    }
  }
  function showMenu(anchor, menu) {
    hideMenu();
    const box = el("div", "panel");
    box.append(el("div", "title", menu.title));
    if (menu.message)
      box.append(el("div", "message muted-text", menu.message));
    for (const item of menu.rows) {
      const row = el("button", "row", item.label);
      row.type = "button";
      if (item.sub)
        row.append(el("span", "sub muted-text", item.sub));
      row.addEventListener("mousedown", (e) => e.preventDefault());
      row.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        hideMenu();
        item.onSelect();
      });
      box.append(row);
    }
    shadow().append(box);
    const r = anchor.getBoundingClientRect();
    const width = Math.max(box.offsetWidth, 240);
    box.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - width - 8))}px`;
    const below = r.bottom + 6;
    box.style.top = `${below + box.offsetHeight > window.innerHeight ? Math.max(8, r.top - box.offsetHeight - 6) : below}px`;
    panel = box;
    dismiss = (e) => {
      if (e instanceof KeyboardEvent && e.key !== "Escape")
        return;
      if (e instanceof MouseEvent && e.composedPath().includes(box))
        return;
      hideMenu();
    };
    document.addEventListener("mousedown", dismiss, true);
    document.addEventListener("keydown", dismiss, true);
    window.addEventListener("scroll", dismiss, true);
  }
  var bar = null;
  function hideBar() {
    bar?.remove();
    bar = null;
  }
  function showSaveBar(options) {
    hideBar();
    const box = el("div", "bar");
    const host = (() => {
      try {
        return new URL(options.origin).hostname;
      } catch {
        return options.origin;
      }
    })();
    const text = el("div", "");
    text.append(document.createTextNode("Save this login to "), el("b", "", "LocalPass"), document.createTextNode("?"));
    const detail = el("div", "muted-text", `${options.username || "(no username)"} on ${host}`);
    detail.style.marginTop = "4px";
    const buttons = el("div", "buttons");
    const save = el("button", "btn primary", "Save");
    const later = el("button", "btn", "Not now");
    const never = el("button", "btn", "Never for this site");
    for (const b of [save, later, never])
      b.type = "button";
    save.addEventListener("click", async () => {
      save.disabled = true;
      detail.textContent = "Saving…";
      try {
        detail.textContent = await options.onSave();
      } catch (e) {
        detail.textContent = e instanceof Error ? e.message : "Could not save";
      }
      buttons.remove();
      setTimeout(hideBar, 2500);
    });
    later.addEventListener("click", () => {
      options.onDismiss(false);
      hideBar();
    });
    never.addEventListener("click", () => {
      options.onDismiss(true);
      hideBar();
    });
    buttons.append(save, later, never);
    box.append(text, detail, buttons);
    shadow().append(box);
    bar = box;
  }
  function showSheet(title, onClose) {
    const backdrop = el("div", "backdrop");
    const box = el("div", "sheet");
    const heading = el("h3", "", title);
    const body = el("div", "");
    const rows = el("div", "rows");
    const buttons = el("div", "buttons");
    box.append(heading, body, rows, buttons);
    backdrop.append(box);
    shadow().append(backdrop);
    let closed = false;
    const close = () => {
      if (closed)
        return;
      closed = true;
      backdrop.remove();
      document.removeEventListener("keydown", onKey, true);
    };
    const dismissed = () => {
      close();
      onClose();
    };
    const onKey = (e) => {
      if (e.key === "Escape")
        dismissed();
    };
    document.addEventListener("keydown", onKey, true);
    backdrop.addEventListener("mousedown", (e) => {
      if (e.target === backdrop)
        dismissed();
    });
    return {
      setBody(lines) {
        body.replaceChildren(...lines.map((line, i) => el("p", i ? "muted-text" : "", line)));
      },
      setRows(items) {
        rows.replaceChildren(...items.map((item) => {
          const row = el("button", "row", item.label);
          row.type = "button";
          if (item.sub)
            row.append(el("span", "sub muted-text", item.sub));
          row.addEventListener("click", item.onSelect);
          return row;
        }));
      },
      setButtons(list) {
        buttons.replaceChildren(...list.map((b) => {
          const button = el("button", b.primary ? "btn primary" : "btn", b.label);
          button.type = "button";
          button.addEventListener("click", b.onClick);
          return button;
        }));
      },
      close
    };
  }

  // src/content/passkeys.ts
  var UNAVAILABLE = new Set(["unpaired", "appNotRunning", "disconnected", "denied", "untrusted", "error", "noVault", "offline", "timeout"]);
  var open = new Map;
  var conditional = null;
  function post(id, body) {
    window.postMessage({ __lp: "res", id, ...body }, "*");
  }
  function finish(id, body) {
    const entry = open.get(id);
    if (!entry && !(conditional?.id === id))
      return;
    open.delete(id);
    entry?.sheet?.close();
    if (conditional?.id === id)
      conditional = null;
    post(id, body);
  }
  function failure(e) {
    const code = e instanceof BrokerError ? e.code : "internal";
    const message = e instanceof Error ? e.message : String(e);
    const name = code === "passkeyExists" ? "InvalidStateError" : code === "unsupported" ? "NotSupportedError" : code === "invalid" ? "SecurityError" : "NotAllowedError";
    return { ok: false, name, message };
  }
  function describe(c) {
    return { label: c.userName || c.userDisplayName || "Passkey", sub: `${c.rpName || c.rpId}${c.createdMs ? ` · ${new Date(c.createdMs).toLocaleDateString()}` : ""}` };
  }
  async function enabled() {
    try {
      const stored = await chrome.storage.local.get("passkeysEnabled");
      return stored.passkeysEnabled !== false;
    } catch {
      return true;
    }
  }
  async function usable() {
    if (!await enabled())
      return false;
    try {
      const reply = await ask({ kind: "findLogins", interactive: true });
      return reply.status === "ok" || reply.status === "locked";
    } catch {
      return false;
    }
  }
  async function startCreate(id, p) {
    if (!await usable())
      return post(id, { fallback: true });
    const site = p.rpName || p.rpId || location.hostname;
    const cancelled = { ok: false, name: "NotAllowedError", message: "The user cancelled the request." };
    const sheet = showSheet("Create a passkey?", () => finish(id, cancelled));
    open.set(id, { sheet });
    sheet.setBody([site, p.userName ? `Account: ${p.userName}` : "The passkey is saved in your LocalPass vault and syncs to your other devices."]);
    const create = async () => {
      sheet.setBody(["Waiting for LocalPass…", "Confirm with Windows Hello if it asks."]);
      sheet.setButtons([{ label: "Cancel", onClick: () => finish(id, cancelled) }]);
      try {
        const result = await ask({ kind: "passkeyCreate", request: p });
        finish(id, { ok: true, result });
      } catch (e) {
        const outcome = failure(e);
        sheet.setBody(["Could not create the passkey.", outcome.message]);
        sheet.setButtons([
          { label: "Use another method", onClick: () => finish(id, { fallback: true }) },
          { label: "Close", onClick: () => finish(id, outcome) }
        ]);
      }
    };
    sheet.setButtons([
      { label: "Create passkey", primary: true, onClick: () => void create() },
      { label: "Use another method", onClick: () => finish(id, { fallback: true }) }
    ]);
  }
  async function sign(id, p, candidate) {
    const result = await ask({ kind: "passkeyAssert", itemId: candidate.itemId, rpId: p.rpId, challenge: p.challenge, uv: p.uv });
    finish(id, { ok: true, result });
  }
  async function startGet(id, p) {
    if (!await enabled())
      return post(id, { fallback: true });
    if (p.conditional)
      return startConditional(id, p);
    if (!await usable())
      return post(id, { fallback: true });
    const cancelled = { ok: false, name: "NotAllowedError", message: "The user cancelled the request." };
    const sheet = showSheet("Sign in with a passkey", () => finish(id, cancelled));
    open.set(id, { sheet });
    sheet.setBody(["Looking for your passkeys…"]);
    const other = { label: "Use another method", onClick: () => finish(id, { fallback: true }) };
    const load = async () => {
      let found;
      try {
        found = await ask({ kind: "passkeyCandidates", rpId: p.rpId, allow: p.allow });
      } catch (e) {
        const code = e instanceof BrokerError ? e.code : "internal";
        if (code === "locked") {
          sheet.setBody(["LocalPass is locked.", "Unlock it to use your passkeys."]);
          sheet.setButtons([
            { label: "Unlock", primary: true, onClick: () => void ask({ kind: "unlock" }).then(load, () => load()) },
            other
          ]);
          return;
        }
        if (UNAVAILABLE.has(code))
          return finish(id, { fallback: true });
        return finish(id, failure(e));
      }
      if (!found.length)
        return finish(id, { fallback: true });
      sheet.setBody([`Sign in to ${p.rpId}`, "Choose a passkey:"]);
      sheet.setRows(found.map((c) => ({
        ...describe(c),
        onSelect: () => {
          sheet.setRows([]);
          sheet.setBody(["Waiting for LocalPass…", "Confirm with Windows Hello if it asks."]);
          sheet.setButtons([{ label: "Cancel", onClick: () => finish(id, cancelled) }]);
          sign(id, p, c).catch((e) => {
            const outcome = failure(e);
            sheet.setBody(["Could not sign in.", outcome.message]);
            sheet.setButtons([other, { label: "Close", onClick: () => finish(id, outcome) }]);
          });
        }
      })));
      sheet.setButtons([other]);
    };
    await load();
  }
  function startConditional(id, p) {
    if (conditional)
      post(conditional.id, { ok: false, name: "AbortError", message: "Replaced by a newer request." });
    conditional = { id, payload: p };
  }
  function conditionalPending() {
    return conditional !== null;
  }
  async function conditionalCandidates() {
    if (!conditional)
      return [];
    try {
      return await ask({ kind: "passkeyCandidates", rpId: conditional.payload.rpId, allow: conditional.payload.allow });
    } catch {
      return [];
    }
  }
  function passkeyRow(candidate) {
    const text = describe(candidate);
    return {
      label: `Passkey: ${text.label}`,
      sub: text.sub,
      onSelect: () => void completeConditional(candidate)
    };
  }
  async function completeConditional(candidate) {
    const current = conditional;
    if (!current)
      return;
    try {
      await sign(current.id, current.payload, candidate);
    } catch (e) {
      console.warn("passkey sign-in failed", e);
    }
  }
  function installPasskeyBridge() {
    window.addEventListener("message", (event) => {
      if (event.source !== window)
        return;
      const data = event.data;
      if (!data || data.__lp !== "req" || typeof data.id !== "string")
        return;
      if (data.op === "create")
        startCreate(data.id, data.payload).catch(() => post(data.id, { fallback: true }));
      else if (data.op === "get")
        startGet(data.id, data.payload).catch(() => post(data.id, { fallback: true }));
      else if (data.op === "cancel") {
        const target = data.payload.target;
        open.get(target)?.sheet?.close();
        open.delete(target);
        if (conditional?.id === target)
          conditional = null;
      }
    });
  }

  // src/content/index.ts
  installPasskeyBridge();
  var forms = [];
  var decorated = new WeakSet;
  var lastRescan = 0;
  function formOf(field) {
    if (!field)
      return;
    const known = forms.find((f) => f.root.contains(field));
    if (known)
      return known;
    const now = Date.now();
    if (now - lastRescan < 250)
      return;
    lastRescan = now;
    scan();
    return forms.find((f) => f.root.contains(field));
  }
  function applyFill(form, fill) {
    if (form.username && fill.username)
      setValue(form.username, fill.username);
    if (form.password && fill.password)
      setValue(form.password, fill.password);
    if (form.otp && fill.totpCode)
      setValue(form.otp, fill.totpCode);
  }
  function bestForm() {
    return formOf(document.activeElement) ?? forms.find((f) => f.password) ?? forms[0];
  }
  async function openMenu(field, form) {
    let reply;
    try {
      reply = await ask({ kind: "findLogins", interactive: true });
    } catch {
      reply = { status: "error", logins: [] };
    }
    const reopen = () => void openMenu(field, form);
    const title = "LocalPass";
    const note = (message) => showMenu(field, { title, message, rows: [] });
    switch (reply.status) {
      case "ok":
        break;
      case "locked":
        return showMenu(field, {
          title,
          message: "Your vault is locked.",
          rows: [
            {
              label: "Unlock LocalPass",
              onSelect: () => void ask({ kind: "unlock" }).then(reopen).catch(() => note("LocalPass was not unlocked."))
            }
          ]
        });
      case "noVault":
        return note("Create or join a vault in the LocalPass app first.");
      case "unpaired":
      case "denied":
        return note("Click the LocalPass icon in the browser toolbar and choose Connect.");
      case "untrusted":
        return note("Something answered that could not prove it is LocalPass. Nothing was shared.");
      default:
        return note("Can't reach LocalPass. Check that the app is running, then try again.");
    }
    const rows = reply.logins.map((login) => ({
      label: login.title,
      sub: login.username || "(no username)",
      onSelect: () => void ask({ kind: "fill", itemId: login.id }).then((fill) => applyFill(form, fill)).catch((e) => note(e instanceof Error ? e.message : "Could not fill"))
    }));
    if (form.isNewPassword) {
      rows.push({
        label: "Generate a strong password",
        sub: "Fills the new password boxes",
        onSelect: () => void ask({ kind: "generate" }).then(({ password }) => {
          for (const box of [form.password, ...form.extraPasswords])
            if (box)
              setValue(box, password);
        }).catch(() => {})
      });
    }
    if (conditionalPending()) {
      for (const candidate of await conditionalCandidates())
        rows.unshift(passkeyRow(candidate));
    }
    if (!rows.length)
      return note("No saved logins for this site yet.");
    showMenu(field, { title, rows });
  }
  function onFocus(field, form) {
    hideMenu();
    ask({ kind: "findLogins", interactive: false }).then((reply) => {
      if (document.activeElement !== field)
        return;
      const useful = reply.status === "ok" && (reply.logins.length > 0 || form.isNewPassword || conditionalPending());
      showBadge(field, !useful, () => void openMenu(field, form));
    }).catch(() => {});
  }
  function scan() {
    forms = findLoginForms(document);
    for (const form of forms) {
      for (const field of [form.username, form.password, form.otp]) {
        if (!field || decorated.has(field))
          continue;
        decorated.add(field);
        field.addEventListener("focus", () => onFocus(field, form));
        field.addEventListener("blur", () => setTimeout(() => !document.hasFocus() || hideBadgeIfAway(), 200));
        if (document.activeElement === field)
          onFocus(field, form);
      }
    }
  }
  function hideBadgeIfAway() {
    const active = document.activeElement;
    if (!(active instanceof HTMLInputElement) || !formOf(active))
      hideBadge();
  }
  var lastCapture = "";
  function capture(form) {
    const password = form.password?.value ?? "";
    if (!password)
      return;
    const username = form.username?.value ?? "";
    const key = `${username}\x00${password}`;
    if (key === lastCapture)
      return;
    lastCapture = key;
    ask({ kind: "capture", username, password }).catch(() => {});
    if (window === window.top) {
      setTimeout(() => {
        if (form.password && !form.password.isConnected)
          checkPending();
      }, 1500);
    }
  }
  function watchSubmissions() {
    document.addEventListener("submit", (e) => {
      const form = formOf(e.target);
      if (form)
        capture(form);
    }, true);
    document.addEventListener("click", (e) => {
      const button = e.target?.closest?.('button, input[type="submit"], [role="button"]');
      const form = formOf(button ?? null);
      if (form)
        capture(form);
    }, true);
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Enter")
        return;
      const form = formOf(e.target);
      if (form)
        capture(form);
    }, true);
  }
  async function checkPending() {
    if (window !== window.top)
      return;
    let pending = null;
    try {
      pending = await ask({ kind: "pendingSave" });
    } catch {
      return;
    }
    if (pending)
      offerSave(pending);
  }
  function offerSave(pending) {
    showSaveBar({
      origin: pending.origin,
      username: pending.username,
      onSave: async () => {
        const outcome = await ask({ kind: "savePending", id: pending.id });
        return outcome.status === "created" ? "Saved to LocalPass." : outcome.status === "updated" ? "Password updated in LocalPass." : "Already saved.";
      },
      onDismiss: (never) => void ask({ kind: "dismissPending", id: pending.id, never }).catch(() => {})
    });
  }
  chrome.runtime.onMessage.addListener((message) => {
    if (message.kind === "doFill") {
      const form = bestForm();
      if (form)
        applyFill(form, message.fill);
    }
    return false;
  });
  var scanTimer;
  new MutationObserver(() => {
    clearTimeout(scanTimer);
    scanTimer = setTimeout(scan, 300);
  }).observe(document.documentElement, { childList: true, subtree: true });
  scan();
  watchSubmissions();
  var ready = () => {
    scan();
    checkPending();
  };
  if (document.readyState === "loading")
    document.addEventListener("DOMContentLoaded", ready, { once: true });
  else
    ready();
  window.addEventListener("pagehide", () => {
    hideMenu();
    hideBar();
  });
})();
