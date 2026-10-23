// The content script: finds login forms, offers LocalPass logins next to them, fills on request,
// and notices sign-ins so they can be saved. It never sees secrets until the user picks a login,
// and the service worker decides which site a request is for.

import type { FindLoginsReply, PendingSave, ToContent } from "../shared/messages";
import type { LoginFill, LoginMatch, SaveOutcome } from "../shared/protocol";
import { MAX_CAPTURE_CHARS } from "../shared/limits";
import { ask } from "./ask";
import { type LoginForm, findLoginForms, setValue } from "./detect";
import { conditionalCandidates, conditionalPending, installPasskeyBridge, passkeyRow } from "./passkeys";
import { type MenuRow, hideBadge, hideBar, hideMenu, showBadge, showMenu, showSaveBar } from "./ui";

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

// ---- the badge and menu ----

async function openMenu(field: HTMLInputElement, form: LoginForm): Promise<void> {
  let reply: FindLoginsReply;
  try {
    reply = await ask<FindLoginsReply>({ kind: "findLogins", interactive: true });
  } catch {
    reply = { status: "error", logins: [] };
  }
  const reopen = () => void openMenu(field, form);
  const title = "LocalPass";
  const note = (message: string) => showMenu(field, { title, message, rows: [] });
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
            onSelect: () =>
              void ask({ kind: "unlock" })
                .then(reopen)
                .catch(() => note("LocalPass was not unlocked.")),
          },
        ],
      });
    case "noVault":
      return note("Create or join a vault in the LocalPass app first.");
    case "unpaired":
    case "denied":
      return note("Click the LocalPass icon in the browser toolbar and choose Connect.");
    case "untrusted":
      return note("Something answered that could not prove it is LocalPass. Nothing was shared.");
    case "tooMany":
      return note("LocalPass is busy right now. Try again in a moment.");
    case "tooLarge":
      return note("LocalPass could not send that answer (too much data).");
    default:
      return note("Can't reach LocalPass. Check that the app is running, then try again.");
  }

  const rows: MenuRow[] = reply.logins.map((login: LoginMatch) => ({
    label: login.title,
    sub: login.username || "(no username)",
    onSelect: () =>
      void ask<LoginFill>({ kind: "fill", itemId: login.id })
        .then((fill) => applyFill(form, fill))
        .catch((e: unknown) => note(e instanceof Error ? e.message : "Could not fill")),
  }));
  if (form.isNewPassword) {
    rows.push({
      label: "Generate a strong password",
      sub: "Fills the new password boxes",
      onSelect: () =>
        void ask<{ password: string }>({ kind: "generate" })
          .then(({ password }) => {
            for (const box of [form.password, ...form.extraPasswords]) if (box) setValue(box, password);
          })
          .catch(() => {}),
    });
  }
  // A page waiting for a passkey sign-in (conditional mediation) gets its passkeys listed here too.
  if (conditionalPending()) {
    for (const candidate of await conditionalCandidates()) rows.unshift(passkeyRow(candidate));
  }
  if (!rows.length) return note("No saved logins for this site yet.");
  showMenu(field, { title, rows });
}

function onFocus(field: HTMLInputElement, form: LoginForm): void {
  hideMenu();
  void ask<FindLoginsReply>({ kind: "findLogins", interactive: false })
    .then((reply) => {
      if (document.activeElement !== field) return;
      const useful = reply.status === "ok" && (reply.logins.length > 0 || form.isNewPassword || conditionalPending());
      // Muted when LocalPass can't help yet (locked or not connected); the click explains why.
      showBadge(field, !useful, () => void openMenu(field, form));
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

function capture(form: LoginForm): void {
  const password = form.password?.value ?? "";
  if (!password) return;
  const username = form.username?.value ?? "";
  if (username.length > MAX_CAPTURE_CHARS || password.length > MAX_CAPTURE_CHARS) return; // not a sign-in a person typed
  const key = `${username}\0${password}`;
  if (key === lastCapture) return;
  lastCapture = key;
  void ask({ kind: "capture", username, password }).catch(() => {});
  // Single-page sites don't navigate: if the form is gone a moment later, the sign-in worked.
  if (window === window.top) {
    setTimeout(() => {
      if (form.password && !form.password.isConnected) void checkPending();
    }, 1500);
  }
}

function watchSubmissions(): void {
  document.addEventListener(
    "submit",
    (e) => {
      const form = formOf(e.target as Element);
      if (form) capture(form);
    },
    true,
  );
  document.addEventListener(
    "click",
    (e) => {
      const button = (e.target as Element | null)?.closest?.('button, input[type="submit"], [role="button"]');
      const form = formOf(button ?? null);
      if (form) capture(form);
    },
    true,
  );
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key !== "Enter") return;
      const form = formOf(e.target as Element);
      if (form) capture(form);
    },
    true,
  );
}

async function checkPending(): Promise<void> {
  if (window !== window.top) return;
  let pending: PendingSave | null = null;
  try {
    pending = await ask<PendingSave | null>({ kind: "pendingSave" });
  } catch {
    return;
  }
  if (pending) offerSave(pending);
}

function offerSave(pending: PendingSave): void {
  showSaveBar({
    origin: pending.origin,
    username: pending.username,
    onSave: async () => {
      const outcome = await ask<SaveOutcome>({ kind: "savePending", id: pending.id });
      if (outcome.status === "declined") return "You chose not to replace the saved password. Nothing was changed.";
      return outcome.status === "created" ? "Saved to LocalPass." : outcome.status === "updated" ? "Password updated in LocalPass." : "Already saved.";
    },
    onDismiss: (never) => void ask({ kind: "dismissPending", id: pending.id, never }).catch(() => {}),
  });
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
window.addEventListener("pagehide", () => {
  hideMenu();
  hideBar();
});
