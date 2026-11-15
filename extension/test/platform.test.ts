// The texts' view of the platform: the name of the app's "is it you?" check and the browser's name.

import { afterEach, describe, expect, test } from "bun:test";
import { installChrome } from "./chrome";
import { browserName, checkName, osFromNavigator, platformOs, userCheckName } from "../src/shared/platform";

const CHROME_LINUX = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";
const brands = (...names: string[]) => names.map((brand) => ({ brand }));

describe("the check's name", () => {
  test("Windows Hello only on Windows", () => {
    expect(checkName("win")).toBe("Windows Hello");
    for (const os of ["linux", "mac", "cros", "openbsd", ""]) expect(checkName(os)).toBe("your master password");
  });

  test("extension pages and the worker ask getPlatformInfo", async () => {
    const fake = installChrome();
    fake.os = "linux";
    expect(await platformOs()).toBe("linux");
    expect(await userCheckName()).toBe("your master password");
    fake.os = "win";
    expect(await userCheckName()).toBe("Windows Hello");
  });

  test("a content script (no getPlatformInfo) reads navigator", async () => {
    const fake = installChrome();
    delete (globalThis as unknown as { chrome: { runtime: Record<string, unknown> } }).chrome.runtime.getPlatformInfo;
    void fake;
    expect(await platformOs()).toBe(osFromNavigator());
  });

  test("navigator platforms map to getPlatformInfo's names", () => {
    expect(osFromNavigator({ userAgent: "", platform: "Win32" })).toBe("win");
    expect(osFromNavigator({ userAgent: "", platform: "Linux x86_64" })).toBe("linux");
    expect(osFromNavigator({ userAgent: "", platform: "MacIntel" })).toBe("mac");
    expect(osFromNavigator({ userAgent: "", platform: "Linux armv8l", userAgentData: { platform: "Android" } })).toBe("android");
    expect(osFromNavigator({ userAgent: "", platform: "", userAgentData: { platform: "Windows" } })).toBe("win");
    expect(osFromNavigator({ userAgent: "", platform: "Linux x86_64", userAgentData: { platform: "Chrome OS" } })).toBe("cros");
    expect(osFromNavigator({ userAgent: "" })).toBe("");
  });
});

describe("the browser's name", () => {
  test("from the user agent and its brands", () => {
    expect(browserName({ userAgent: "Mozilla/5.0 (X11; Linux x86_64; rv:160.0) Gecko/20100101 Firefox/160.0" })).toBe("Firefox");
    expect(browserName({ userAgent: CHROME_LINUX, userAgentData: { brands: brands("Chromium", "Google Chrome", "Not=A?Brand") } })).toBe("Chrome");
    expect(browserName({ userAgent: CHROME_LINUX, userAgentData: { brands: brands("Chromium", "Not=A?Brand") } })).toBe("Chromium");
    expect(browserName({ userAgent: CHROME_LINUX, userAgentData: { brands: brands("Chromium", "Brave", "Not=A?Brand") } })).toBe("Brave");
    expect(browserName({ userAgent: `${CHROME_LINUX} Edg/157.0.0.0`, userAgentData: { brands: brands("Chromium", "Microsoft Edge") } })).toBe("Microsoft Edge");
    expect(browserName({ userAgent: `${CHROME_LINUX} OPR/120.0.0.0` })).toBe("Opera");
    expect(browserName({ userAgent: `${CHROME_LINUX} Vivaldi/7.5` })).toBe("Vivaldi");
    // No brands at all (older Chrome): Chrome.
    expect(browserName({ userAgent: CHROME_LINUX })).toBe("Chrome");
  });
});

afterEach(() => {
  installChrome();
});
