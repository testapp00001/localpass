// Size limits for what a web page may hand to a passkey call, and helpers for showing page text.
// The app enforces its own limits (crates/lp-engine/src/credentials.rs); these are the same
// numbers, applied first, so a hostile page cannot make the extension build an oversized request
// (the native-messaging helper drops a frame over 1 MiB). A call over a limit is not refused: it
// goes back to the browser's own passkey flow.

/** Longest user-facing text (rp name, user name, display name), in characters. */
export const MAX_TEXT_CHARS = 256;
/** Most entries in an exclude or allow list, or in the accepted algorithm list. */
export const MAX_LIST = 256;
/** Longest credential id (WebAuthn's own limit). */
export const MAX_CREDENTIAL_ID_BYTES = 1023;
/** Longest user handle (WebAuthn's own limit). */
export const MAX_USER_HANDLE_BYTES = 64;
/** Longest challenge accepted (real relying parties use 16 to a few hundred bytes). */
export const MAX_CHALLENGE_BYTES = 4096;
/** Longest relying party id (a DNS name). */
export const MAX_RP_ID_CHARS = 253;
/** Longest user name or password taken from a sign-in form to offer saving (a page can set any size). */
export const MAX_CAPTURE_CHARS = 4096;

/** Whether `rpId` is the page's host or a parent domain of it (a literal IP address must match exactly). */
export function rpIdFits(rpId: string, host: string): boolean {
  if (!rpId) return true;
  const a = rpId.toLowerCase();
  const b = host.toLowerCase();
  if (b === a) return true;
  if (/^[\d.]+$/.test(b) || b.includes(":")) return false; // an IP address has no parent domain
  return b.endsWith(`.${a}`);
}

/** The size in bytes of a base64url string (unpadded), without decoding it. */
export function b64uBytes(text: string): number {
  return Math.floor((text.length * 3) / 4);
}

export function isB64u(text: unknown): text is string {
  return typeof text === "string" && /^[A-Za-z0-9_-]*$/.test(text);
}

// Control characters, and line and paragraph separators: shown as a space.
const CONTROL = new RegExp("[\\u0000-\\u001f\\u007f-\\u009f\\u2028\\u2029]", "g");
// Characters with no width, or that reorder or hide the text around them: dropped. Every format (Cf) character of
// Unicode 17 (soft hyphen, number signs and marks that join the text after them, zero-width and direction marks,
// bidi embeddings and isolates, invisible operators, the byte order mark, the interlinear annotation marks,
// Egyptian hieroglyph and shorthand format controls, musical beams, the tag characters), plus the combining
// grapheme joiner and the Hangul fillers (blank letters). The same set as `is_invisible` in lp-broker's server.rs,
// which drops the line and paragraph separators too (here `CONTROL` turns them into spaces).
const INVISIBLE = new RegExp(
  "[\\u{00ad}\\u{034f}\\u{0600}-\\u{0605}\\u{061c}\\u{06dd}\\u{070f}\\u{0890}-\\u{0891}\\u{08e2}\\u{115f}-\\u{1160}\\u{180e}" +
    "\\u{200b}-\\u{200f}\\u{202a}-\\u{202e}\\u{2060}-\\u{206f}\\u{3164}\\u{feff}\\u{ffa0}\\u{fff9}-\\u{fffb}" +
    "\\u{110bd}\\u{110cd}\\u{13430}-\\u{1343f}\\u{1bca0}-\\u{1bca3}\\u{1d173}-\\u{1d17a}\\u{e0000}-\\u{e007f}]",
  "gu",
);

/**
 * Text from a page, made safe to show: control characters and the invisible characters that
 * reorder or hide text are dropped, runs of white space collapse, and it is cut to `max`
 * characters. It is only ever shown as text (never as HTML).
 */
export function displayText(text: string, max = 80): string {
  const clean = text.replace(CONTROL, " ").replace(INVISIBLE, "").replace(/\s+/g, " ").trim();
  const chars = [...clean];
  return chars.length > max ? `${chars.slice(0, max - 1).join("")}…` : clean;
}
