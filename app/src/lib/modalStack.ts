// The open dialogs, so that Esc (and Tab) act on the topmost one only. Pure: the caller says
// how two entries are ordered on screen (for the app: document order, since every dialog has
// the same z-index and the later one in the document is painted on top).

export interface ModalStack<T> {
  /** Adds an open dialog; the returned function removes it again (safe to call twice). */
  open(entry: T): () => void;
  /** The dialog on top, or undefined when none is open. */
  top(): T | undefined;
  size(): number;
}

/**
 * `above(a, b)` is true when `a` is drawn over `b`, false when it is drawn under it, and
 * undefined when unknown; then the one opened later counts as on top.
 */
export function createModalStack<T>(above: (a: T, b: T) => boolean | undefined = () => undefined): ModalStack<T> {
  const entries: T[] = [];
  return {
    open(entry) {
      entries.push(entry);
      let removed = false;
      return () => {
        if (removed) return;
        removed = true;
        const i = entries.lastIndexOf(entry);
        if (i >= 0) entries.splice(i, 1);
      };
    },
    top() {
      let best: T | undefined;
      for (const e of entries) {
        if (best === undefined || above(e, best) !== false) best = e;
      }
      return best;
    },
    size: () => entries.length,
  };
}

/** Where Tab moves within a dialog with `count` focusable controls (`index` -1: focus is outside). */
export function wrapFocus(count: number, index: number, back: boolean): number | undefined {
  if (count === 0) return undefined;
  if (index < 0) return back ? count - 1 : 0;
  if (back && index === 0) return count - 1;
  if (!back && index === count - 1) return 0;
  return undefined; // the browser's own Tab stays inside
}
