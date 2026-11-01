import { describe, expect, test } from "bun:test";
import type { DuplicateItem } from "../src/lib/api";
import { lostWithTrash } from "../src/lib/duplicates";

const item = (patch: Partial<DuplicateItem> = {}): DuplicateItem => ({
  id: "i",
  kind: "login",
  title: "t",
  subtitle: "",
  url: null,
  favorite: false,
  folder: null,
  hasPassword: true,
  hasTotp: false,
  trashed: false,
  updatedMs: 0,
  conflicted: false,
  urls: [],
  hasNotes: false,
  customFields: 0,
  updatedBy: "",
  ...patch,
});

describe("what moving duplicates to the trash would lose", () => {
  test("nothing when the other copies have nothing extra", () => {
    expect(lostWithTrash(item(), [item(), item()])).toEqual([]);
  });
  test("names a 2FA secret, notes and custom fields the kept copy lacks", () => {
    expect(lostWithTrash(item(), [item({ hasTotp: true, hasNotes: true, customFields: 2 })])).toEqual(["a 2FA secret", "notes", "custom fields"]);
  });
  test("a copy with extras is fine when the kept one has them too", () => {
    const rich = { hasTotp: true, hasNotes: true, customFields: 2 };
    expect(lostWithTrash(item(rich), [item(rich)])).toEqual([]);
  });
  test("looks at every copy that goes, not only the first", () => {
    expect(lostWithTrash(item(), [item(), item({ hasNotes: true })])).toEqual(["notes"]);
  });
  test("more custom fields on a trashed copy count even when the kept one has some", () => {
    expect(lostWithTrash(item({ customFields: 1 }), [item({ customFields: 3 })])).toEqual(["custom fields"]);
    expect(lostWithTrash(item({ customFields: 3 }), [item({ customFields: 1 })])).toEqual([]);
  });
});
