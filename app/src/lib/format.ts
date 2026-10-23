export function timeAgo(ms: number | null | undefined): string {
  if (!ms) return "never";
  const secs = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (secs < 45) return "just now";
  if (secs < 3600) return `${Math.round(secs / 60)} min ago`;
  if (secs < 86400) return `${Math.round(secs / 3600)} h ago`;
  const days = Math.round(secs / 86400);
  return days === 1 ? "yesterday" : `${days} days ago`;
}

export function formatDate(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export function platformLabel(p: string): string {
  switch (p) {
    case "windows":
      return "Windows";
    case "android":
      return "Android";
    case "macos":
      return "macOS";
    case "linux":
      return "Linux";
    case "ios":
      return "iOS";
    default:
      return p || "Unknown";
  }
}

export function hostOf(url: string): string {
  try {
    const u = new URL(url.includes("://") ? url : `https://${url}`);
    return u.hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** Ensures a URL opens in a browser (adds https:// when missing). */
export function openableUrl(url: string): string | null {
  const trimmed = url.trim();
  if (!trimmed) return null;
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`;
  return /^https?:\/\//i.test(withScheme) ? withScheme : null;
}
