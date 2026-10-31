import { GlobalRegistrator } from "@happy-dom/global-registrator";
import { guard } from "../src/content/ui";

// A DOM for the form-detection tests.
GlobalRegistrator.register();

// The page interface's shadow root is open in tests (as in the e2e build), so they can read it.
(globalThis as unknown as { __E2E__: boolean }).__E2E__ = true;
// Unit tests run the Chrome build; the Firefox storage backend is tested through `indexedDbArea` directly.
(globalThis as unknown as { __FIREFOX__: boolean }).__FIREFOX__ = false;

/**
 * happy-dom has no Intersection Observer v2, which the page interface uses to prove that a click
 * could be seen (`guard` in content/ui.ts). This stand-in reports every observed control as plainly
 * visible at once, and a test can change that with `FakeVisibility.set` / `report`.
 */
export class FakeVisibility {
  static visible = true;
  static observers: FakeVisibility[] = [];
  private targets = new Set<Element>();
  constructor(private callback: (entries: unknown[], observer: unknown) => void) {
    FakeVisibility.observers.push(this);
  }
  observe(target: Element) {
    this.targets.add(target);
    this.callback([entry(target, FakeVisibility.visible)], this);
  }
  unobserve(target: Element) {
    this.targets.delete(target);
  }
  disconnect() {
    this.targets.clear();
  }
  /** Reports `target` to the interface, as the browser does when its visibility changes. */
  static report(target: Element, isVisible: boolean) {
    for (const o of FakeVisibility.observers) if (o.targets.has(target)) o.callback([entry(target, isVisible)], o);
  }
}
const entry = (target: Element, isVisible: boolean) => ({ target, isVisible, isIntersecting: isVisible, intersectionRatio: isVisible ? 1 : 0 });
(globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = FakeVisibility;

// Tests click at once, with synthetic events: no waiting and no trusted-click check (the guard's own
// test turns both back on).
guard.holdMs = 0;
guard.needTrusted = false;
