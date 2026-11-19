/** Pure target validation, shared by the ADB helpers and their safety tests. */
export const AUDIT_PACKAGE = "app.localpass.audit";
export const ANDROID_NAMESPACE = "app.localpass";

export interface AndroidTarget {
  serial: string;
  package: string;
}

export function androidTarget(env: Record<string, string | undefined>): AndroidTarget {
  if (env.LOCALPASS_PROFILE !== "audit") {
    throw new Error("Android tests require LOCALPASS_PROFILE=audit");
  }
  const serial = env.LP_ADB_SERIAL?.trim();
  if (!serial || /\s/.test(serial)) throw new Error("Set an explicit LP_ADB_SERIAL");
  if (env.LP_ANDROID_PACKAGE !== AUDIT_PACKAGE) {
    throw new Error(`Android tests require LP_ANDROID_PACKAGE=${AUDIT_PACKAGE}; production targets are refused`);
  }
  return { serial, package: AUDIT_PACKAGE };
}

export function assertEmulator(properties: string): void {
  if (properties.trim() !== "1") {
    throw new Error("Destructive and no-screen-lock tests require an emulator");
  }
}
