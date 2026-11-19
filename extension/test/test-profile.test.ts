import { expect, test } from "bun:test";
import { testProfile } from "../e2e/test-profile";

test("desktop helpers accept only reserved isolated test profiles", () => {
  for (const profile of ["audit", "audit-phone", "e2e123", "e2e_app-1"]) expect(testProfile(profile)).toBe(profile);
  for (const profile of ["", " ", "default", "production", "dev2", "../audit", "audit/..", "audit\\..", "audit\0"]) {
    expect(() => testProfile(profile)).toThrow();
  }
});
