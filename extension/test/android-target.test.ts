import { describe, expect, test } from "bun:test";
import { androidTarget, assertEmulator } from "../e2e/android-target";

const audit = { LOCALPASS_PROFILE: "audit", LP_ADB_SERIAL: "emulator-5554", LP_ANDROID_PACKAGE: "app.localpass.audit" };

describe("Android audit target", () => {
  test("requires the explicit serial, profile and isolated package", () => {
    expect(androidTarget(audit)).toEqual({ serial: "emulator-5554", package: "app.localpass.audit" });
    for (const key of Object.keys(audit)) expect(() => androidTarget({ ...audit, [key]: undefined })).toThrow();
    for (const pkg of ["app.localpass", "app.localpass.debug", "", "app.localpass.audit.other"]) {
      expect(() => androidTarget({ ...audit, LP_ANDROID_PACKAGE: pkg })).toThrow();
    }
    expect(() => androidTarget({ ...audit, LOCALPASS_PROFILE: "default" })).toThrow();
  });
  test("physical devices cannot run destructive/no-lock cases", () => {
    expect(() => assertEmulator("0")).toThrow();
    expect(() => assertEmulator("")).toThrow();
    expect(() => assertEmulator("1\n")).not.toThrow();
  });
});
