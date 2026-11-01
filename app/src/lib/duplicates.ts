import type { DuplicateItem } from "./api";

/** What the copies about to go to the trash have that the copy being kept does not. */
export function lostWithTrash(kept: DuplicateItem, others: DuplicateItem[]): string[] {
  const lost: string[] = [];
  if (!kept.hasTotp && others.some((o) => o.hasTotp)) lost.push("a 2FA secret");
  if (!kept.hasNotes && others.some((o) => o.hasNotes)) lost.push("notes");
  if (others.some((o) => o.customFields > kept.customFields)) lost.push("custom fields");
  return lost;
}
