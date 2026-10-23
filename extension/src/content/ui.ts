// The small pieces of interface LocalPass draws on web pages: a badge in the field, a menu of
// logins, and the "save this login?" bar. They live in a closed shadow root so the page's scripts
// cannot read what they show, and every control that fills, saves or signs only reacts to a click
// that it can prove the user could see (see `arm`), so a page cannot hide or cover it (clickjacking).

import { displayText } from "../shared/limits";

const STYLE = `
  :host { all: initial; }
  * { box-sizing: border-box; font-family: system-ui, "Segoe UI", Roboto, sans-serif; }
  .badge { position: fixed; width: 22px; height: 22px; border: 0; border-radius: 6px; padding: 0; cursor: pointer;
    background: #4f46e5; color: #fff; font: 700 13px/22px system-ui, sans-serif; text-align: center; box-shadow: 0 1px 3px rgba(0,0,0,.35); }
  .badge.muted { background: #64748b; }
  /* No filter, opacity or transform on a control: the browser would then report it as not plainly visible (see arm below). */
  .badge:hover { background: #6366f1; }
  .badge.muted:hover { background: #94a3b8; }
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
  .sheet p { margin: 0 0 8px; line-height: 1.45; overflow-wrap: anywhere; }
  .sheet p.lead { font-size: 15px; font-weight: 600; }
  .sheet .rows { margin: 8px -6px; }
`;

/** Set by `bun run build --e2e`: tests need to look inside, so that build leaves the root open. */
declare const __E2E__: boolean;

let root: ShadowRoot | null = null;
let hostElement: HTMLElement | null = null;

function shadow(): ShadowRoot {
  if (root && hostElement) {
    // A page that rewrites the document (`document.write`, replacing the root's children) removes the host.
    if (!hostElement.isConnected) document.documentElement.append(hostElement);
    return root;
  }
  const host = document.createElement("div");
  hostElement = host;
  // Inline `!important` beats a page rule that is also `!important` (`opacity: 0 !important`,
  // `transform`, `display: none`, ...), and `all: initial` resets every other property.
  for (const [name, value] of [
    ["all", "initial"],
    ["position", "fixed"],
    ["top", "0"],
    ["left", "0"],
    ["width", "0"],
    ["height", "0"],
    ["z-index", "2147483647"],
  ] as const) {
    host.style.setProperty(name, value, "important");
  }
  root = host.attachShadow({ mode: __E2E__ ? "open" : "closed" });
  const style = document.createElement("style");
  style.textContent = STYLE;
  root.append(style);
  document.documentElement.append(host);
  return root;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

// ---- clicks the user could see ----

/**
 * A page that can run script or inject CSS on a site with a saved login can try to make the user click
 * a LocalPass control without seeing it: hide it, cover it with a decoy that ignores the mouse, move it
 * under the pointer, or show it for an instant. A control made with `arm` ignores clicks until the
 * browser itself reports it plainly visible (Intersection Observer v2 `isVisible`: not covered, not
 * transparent, not transformed or filtered) for `holdMs`, and ignores clicks a script made up. Where
 * the browser cannot report visibility the control stays inert and the popup is the way to fill.
 * `data-ready` marks a control that reacts; the end-to-end tests wait for it.
 */
export const guard = { holdMs: 250, needTrusted: true };

interface Armed {
  control: HTMLElement;
  ready: boolean;
  timer: ReturnType<typeof setTimeout> | undefined;
  /** When the control last moved on screen. */
  movedAt: number;
}

const armed = new WeakMap<Element, Armed>();
let watcher: IntersectionObserver | null | undefined;

function setReady(state: Armed, ready: boolean): void {
  state.ready = ready;
  state.control.toggleAttribute("data-ready", ready);
}

function onVisibility(entries: IntersectionObserverEntry[]): void {
  for (const entry of entries) {
    const state = armed.get(entry.target);
    if (!state) continue;
    const visible = (entry as { isVisible?: boolean }).isVisible === true && entry.intersectionRatio >= 0.5;
    if (!visible) {
      clearTimeout(state.timer);
      state.timer = undefined;
      setReady(state, false);
    } else if (!state.ready && state.timer === undefined) {
      if (guard.holdMs <= 0) {
        setReady(state, true);
        continue;
      }
      state.timer = setTimeout(() => {
        state.timer = undefined;
        setReady(state, true);
      }, guard.holdMs);
    }
  }
}

function observer(): IntersectionObserver | null {
  if (watcher !== undefined) return watcher;
  try {
    // Chrome wants a `delay` of at least 100 ms when it tracks visibility.
    watcher = new IntersectionObserver(onVisibility, { threshold: [0.5, 1], trackVisibility: true, delay: 100 } as IntersectionObserverInit);
  } catch {
    watcher = null;
  }
  return watcher;
}

/** Makes `control` run `action` on a click the user could see. */
function arm(control: HTMLElement, action: () => void): Armed {
  const state: Armed = { control, ready: false, timer: undefined, movedAt: 0 };
  armed.set(control, state);
  observer()?.observe(control);
  control.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (guard.needTrusted && !e.isTrusted) return;
    if (!state.ready || performance.now() - state.movedAt < guard.holdMs) return;
    action();
  });
  return state;
}

// ---- badge in a field ----

let badge: { button: HTMLButtonElement; anchor: HTMLElement; frame: number } | null = null;

export function showBadge(anchor: HTMLInputElement, muted: boolean, onClick: () => void): void {
  hideBadge();
  const button = el("button", muted ? "badge muted" : "badge", "L");
  button.type = "button";
  button.title = "LocalPass";
  button.addEventListener("mousedown", (e) => e.preventDefault()); // keep focus in the field
  const state = arm(button, onClick);
  shadow().append(button);
  const place = () => {
    if (!badge) return;
    if (!anchor.isConnected) return hideBadge();
    const r = anchor.getBoundingClientRect();
    const left = `${Math.round(r.right - 28)}px`;
    const top = `${Math.round(r.top + (r.height - 22) / 2)}px`;
    // The page controls the field, so a badge that has just moved is not clicked on yet.
    if (left !== button.style.left || top !== button.style.top) state.movedAt = performance.now();
    button.style.left = left;
    button.style.top = top;
    badge.frame = requestAnimationFrame(place);
  };
  badge = { button, anchor, frame: 0 };
  place();
}

export function hideBadge(): void {
  if (!badge) return;
  cancelAnimationFrame(badge.frame);
  badge.button.remove();
  badge = null;
}

export function badgeAnchor(): HTMLElement | null {
  return badge?.anchor ?? null;
}

// ---- menu of logins ----

export interface MenuRow {
  label: string;
  sub?: string;
  onSelect: () => void;
}

export interface Menu {
  title: string;
  message?: string;
  rows: MenuRow[];
}

let panel: HTMLElement | null = null;
let dismiss: ((e: Event) => void) | null = null;

export function hideMenu(): void {
  panel?.remove();
  panel = null;
  if (dismiss) {
    document.removeEventListener("mousedown", dismiss, true);
    document.removeEventListener("keydown", dismiss, true);
    window.removeEventListener("scroll", dismiss, true);
    dismiss = null;
  }
}

export function showMenu(anchor: HTMLElement, menu: Menu): void {
  hideMenu();
  const box = el("div", "panel");
  box.append(el("div", "title", menu.title));
  if (menu.message) box.append(el("div", "message muted-text", menu.message));
  for (const item of menu.rows) {
    const row = el("button", "row", item.label);
    row.type = "button";
    if (item.sub) row.append(el("span", "sub muted-text", item.sub));
    row.addEventListener("mousedown", (e) => e.preventDefault());
    arm(row, () => {
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
  dismiss = (e: Event) => {
    if (e instanceof KeyboardEvent && e.key !== "Escape") return;
    // Clicks inside our own shadow tree report the host as their target.
    if (e instanceof MouseEvent && e.composedPath().includes(box)) return;
    hideMenu();
  };
  document.addEventListener("mousedown", dismiss, true);
  document.addEventListener("keydown", dismiss, true);
  window.addEventListener("scroll", dismiss, true);
}

// ---- save bar ----

let bar: HTMLElement | null = null;

export function hideBar(): void {
  bar?.remove();
  bar = null;
}

export interface SaveBar {
  origin: string;
  username: string;
  onSave: () => Promise<string>;
  onDismiss: (never: boolean) => void;
}

export function showSaveBar(options: SaveBar): void {
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
  const detail = el("div", "muted-text", `${displayText(options.username, 60) || "(no username)"} on ${displayText(host, 80)}`);
  detail.style.marginTop = "4px";
  const buttons = el("div", "buttons");
  const save = el("button", "btn primary", "Save");
  const later = el("button", "btn", "Not now");
  const never = el("button", "btn", "Never for this site");
  for (const b of [save, later, never]) b.type = "button";
  arm(save, async () => {
    save.disabled = true;
    detail.textContent = "Saving… (LocalPass may ask you to confirm in the app)";
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

// ---- sheet (a centred dialog, used for passkeys) ----

export interface SheetButton {
  label: string;
  primary?: boolean;
  onClick: () => void;
}

export interface Sheet {
  setBody(lines: string[]): void;
  setRows(rows: MenuRow[]): void;
  setButtons(buttons: SheetButton[]): void;
  close(): void;
}

/** Opens a dialog. `onClose` runs when the user dismisses it (Escape or a click outside). */
export function showSheet(title: string, onClose: () => void): Sheet {
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
    if (closed) return;
    closed = true;
    backdrop.remove();
    document.removeEventListener("keydown", onKey, true);
  };
  const dismissed = () => {
    close();
    onClose();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") dismissed();
  };
  document.addEventListener("keydown", onKey, true);
  backdrop.addEventListener("mousedown", (e) => {
    if (e.target === backdrop) dismissed();
  });

  return {
    setBody(lines) {
      body.replaceChildren(...lines.map((line, i) => el("p", i ? "muted-text" : "lead", line)));
    },
    setRows(items) {
      rows.replaceChildren(
        ...items.map((item) => {
          const row = el("button", "row", item.label);
          row.type = "button";
          if (item.sub) row.append(el("span", "sub muted-text", item.sub));
          arm(row, item.onSelect);
          return row;
        }),
      );
    },
    setButtons(list) {
      buttons.replaceChildren(
        ...list.map((b) => {
          const button = el("button", b.primary ? "btn primary" : "btn", b.label);
          button.type = "button";
          // Only the primary button acts for the user (create, unlock); the others cancel or hand the
          // request back to the browser, which a click the user did not see cannot turn into harm.
          if (b.primary) arm(button, b.onClick);
          else button.addEventListener("click", b.onClick);
          return button;
        }),
      );
    },
    close,
  };
}
