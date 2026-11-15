import { GlobalRegistrator } from "@happy-dom/global-registrator";
import { guard } from "../src/content/ui";

// A DOM for the form-detection tests.
GlobalRegistrator.register();

// The page interface's shadow root is open in tests (as in the e2e build), so they can read it.
(globalThis as unknown as { __E2E__: boolean }).__E2E__ = true;
// Unit tests run the Chrome build; the Firefox storage backend is tested through `indexedDbArea` directly.
(globalThis as unknown as { __FIREFOX__: boolean }).__FIREFOX__ = false;

// Tests click with synthetic events: no trusted-click check (the badge's own test turns it back on).
guard.needTrusted = false;
