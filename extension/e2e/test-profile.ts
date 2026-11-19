/** Test helpers must never fall back to the app's default/production profile. */
export function testProfile(profile: string): string {
  if (!/^(?:audit|e2e)[A-Za-z0-9_-]*$/.test(profile)) {
    throw new Error("Tests require an explicit audit/e2e profile; production profiles and paths are refused");
  }
  return profile;
}
