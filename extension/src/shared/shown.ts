// What LocalPass shows about a request, shared by the in-page dialogs (Chrome) and the decision
// window (Firefox). The first line names the relying party id or site the extension checked, never
// the name a page gave itself; page text is cleaned, shortened and only ever shown as text.

import { displayText } from "./limits";
import type { PasskeyCreateInput } from "./messages";
import type { PasskeyCandidate, SaveOutcome } from "./protocol";

/** What the create dialog says. */
export function createDialogLines(p: PasskeyCreateInput, host: string): string[] {
  const rpId = displayText(p.rpId || host, 253);
  const lines = [rpId];
  const account = displayText(p.userName);
  const display = displayText(p.userDisplayName);
  if (account) lines.push(`Account: ${account}${display && display !== account ? ` (${display})` : ""}`);
  const name = displayText(p.rpName);
  if (name && name.toLowerCase() !== rpId.toLowerCase()) lines.push(`The site calls itself "${name}"`);
  lines.push("The passkey is saved in your LocalPass vault and syncs to your other devices.");
  return lines;
}

/** Every passkey row names the site (its id, as the app verified it) before anything the page called it. */
export function describePasskey(c: PasskeyCandidate): { label: string; sub: string } {
  const site = displayText(c.rpId);
  const name = displayText(c.rpName);
  const named = name && name.toLowerCase() !== site.toLowerCase() ? ` (${name})` : "";
  return {
    label: displayText(c.userName || c.userDisplayName) || "Passkey",
    sub: `${site}${named}${c.createdMs ? ` · ${new Date(c.createdMs).toLocaleDateString()}` : ""}`,
  };
}

/** What the save bar says once the app has answered. */
export function saveMessage(outcome: SaveOutcome): string {
  if (outcome.status === "declined") return "You chose not to replace the saved password. Nothing was changed.";
  return outcome.status === "created" ? "Saved to LocalPass." : outcome.status === "updated" ? "Password updated in LocalPass." : "Already saved.";
}
