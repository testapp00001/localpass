// The one piece of interface LocalPass draws on web pages: a small "L" badge in a login field. It lives
// in a closed shadow root so the page's scripts cannot read it. Its click only asks the background to
// open LocalPass's own panel, where the user picks, saves or confirms (background/decide.ts): nothing
// drawn into a page fills, saves or signs, so a page that hides, covers or restyles the badge can at
// most get the panel opened.

const STYLE = `
  :host { all: initial; }
  .badge { position: fixed; width: 22px; height: 22px; border: 0; border-radius: 6px; padding: 0; cursor: pointer;
    background: #4f46e5; color: #fff; font: 700 13px/22px system-ui, "Segoe UI", Roboto, sans-serif; text-align: center;
    box-shadow: 0 1px 3px rgba(0,0,0,.35); box-sizing: border-box; }
  .badge.muted { background: #64748b; }
  .badge:hover { background: #6366f1; }
  .badge.muted:hover { background: #94a3b8; }
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

/**
 * Tests turn the trusted-click check off (happy-dom cannot make trusted events); its own test turns it on.
 */
export const guard = { needTrusted: true };

/**
 * The badge's click: a click a script made up is ignored; a real one only asks for LocalPass's panel (the
 * background opens it once per request), where the user decides with the panel's own hold on every
 * control that acts. Never use this for a control that acts by itself. `data-ready` marks the badge for
 * the end-to-end tests.
 */
function armOpener(control: HTMLElement, action: () => void): void {
  control.toggleAttribute("data-ready", true);
  control.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (guard.needTrusted && !e.isTrusted) return;
    action();
  });
}

// ---- badge in a field ----

let badge: { button: HTMLButtonElement; anchor: HTMLElement; frame: number; onClick: () => void } | null = null;

export function showBadge(anchor: HTMLInputElement, muted: boolean, onClick: () => void): void {
  const className = muted ? "badge muted" : "badge";
  if (badge && badge.anchor === anchor && badge.button.isConnected) {
    // The same field again (its focus event fires again when its window or tab gets the focus back, often
    // from the very click on the badge): update the badge in place. A new button would replace the one under
    // the pointer between its mousedown and mouseup, and that click would be lost.
    badge.button.className = className;
    badge.onClick = onClick;
    return;
  }
  hideBadge();
  const button = document.createElement("button");
  button.className = className;
  button.textContent = "L";
  button.type = "button";
  button.title = "LocalPass";
  button.addEventListener("mousedown", (e) => e.preventDefault()); // keep focus in the field
  const current = { button, anchor, frame: 0, onClick };
  armOpener(button, () => current.onClick());
  shadow().append(button);
  const place = () => {
    if (badge !== current) return;
    if (!anchor.isConnected) return hideBadge();
    const r = anchor.getBoundingClientRect();
    button.style.left = `${Math.round(r.right - 28)}px`;
    button.style.top = `${Math.round(r.top + (r.height - 22) / 2)}px`;
    current.frame = requestAnimationFrame(place);
  };
  badge = current;
  place();
}

export function hideBadge(): void {
  if (!badge) return;
  cancelAnimationFrame(badge.frame);
  badge.button.remove();
  badge = null;
}
