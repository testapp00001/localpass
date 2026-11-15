// What the texts say about where the extension runs: the name of the app's "is it you?" check
// (Windows Hello on Windows, the master password elsewhere) and the browser's name.

/** The parts of `navigator` read here (also in a service worker). */
export interface NavigatorLike {
  userAgent: string;
  platform?: string;
  userAgentData?: { platform?: string; brands?: { brand: string }[] };
}

const nav = (): NavigatorLike => navigator as unknown as NavigatorLike;

/** How the app confirms it is you on `os` (as `chrome.runtime.getPlatformInfo` names it). */
export function checkName(os: string): string {
  return os === "win" ? "Windows Hello" : "your master password";
}

/** The operating system from `navigator`, named like `getPlatformInfo().os` ("" when unknown). */
export function osFromNavigator(n: NavigatorLike = nav()): string {
  const p = (n.userAgentData?.platform || n.platform || "").toLowerCase();
  if (p.startsWith("win")) return "win";
  if (p.includes("android")) return "android";
  if (p.includes("cros") || p.includes("chrome os")) return "cros";
  if (p.startsWith("mac")) return "mac";
  if (p.includes("linux")) return "linux";
  return "";
}

/**
 * The operating system. Extension pages and the service worker ask `getPlatformInfo`; a content
 * script has no such call and reads its own (isolated, not page-controlled) `navigator`.
 */
export async function platformOs(): Promise<string> {
  const runtime = (globalThis as { chrome?: { runtime?: { getPlatformInfo?: unknown } } }).chrome?.runtime;
  if (typeof runtime?.getPlatformInfo === "function") {
    try {
      return (await chrome.runtime.getPlatformInfo()).os;
    } catch {
      // Fall through to navigator.
    }
  }
  return osFromNavigator();
}

/** The name of the app's "is it you?" check on this system. */
export async function userCheckName(): Promise<string> {
  return checkName(await platformOs());
}

/** The running browser's name for texts ("Chrome", "Firefox", "Microsoft Edge", ...). */
export function browserName(n: NavigatorLike = nav()): string {
  const ua = n.userAgent;
  const brands = (n.userAgentData?.brands ?? []).map((b) => b.brand);
  const has = (brand: string) => brands.includes(brand);
  if (ua.includes("Firefox/")) return "Firefox";
  if (ua.includes("Edg/") || has("Microsoft Edge")) return "Microsoft Edge";
  if (ua.includes("OPR/") || has("Opera")) return "Opera";
  if (ua.includes("Brave") || has("Brave")) return "Brave";
  if (ua.includes("Vivaldi/") || has("Vivaldi")) return "Vivaldi";
  if (has("Google Chrome")) return "Chrome";
  // Chromium reports itself only as "Chromium" among its brands.
  if (has("Chromium")) return "Chromium";
  return "Chrome";
}
