import { afterAll, beforeEach, expect, spyOn, test } from "bun:test";

// Intercept execution before loading the helper: this suite cannot touch a
// device, even when the developer has a production phone connected.
const keys = ["LOCALPASS_PROFILE", "LP_ADB_SERIAL", "LP_ANDROID_PACKAGE"] as const;
const prior = Object.fromEntries(keys.map(key => [key, process.env[key]]));
process.env.LOCALPASS_PROFILE = "audit";
process.env.LP_ADB_SERIAL = "audit-fixture-serial";
process.env.LP_ANDROID_PACKAGE = "app.localpass.audit";
const commands: string[][] = [];
let emulator = "1";
let apkPackage = "app.localpass.audit";
let failSetting = false;
const run = spyOn(Bun, "spawnSync").mockImplementation(((command: string[]) => {
  commands.push(command);
  let output = "";
  let exitCode = 0;
  const args = command.slice(3);
  if (command.includes("application-id")) output = apkPackage;
  else if (args.join(" ") === "shell getprop ro.kernel.qemu") output = emulator;
  else if (args.join(" ") === "shell settings get secure autofill_service") output = "app.original/.Service";
  else if (args.join(" ") === "forward --list") output = "audit-fixture-serial tcp:9333 localabstract:original\nother-device tcp:9333 tcp:8";
  else if (failSetting && args.join(" ").startsWith("shell settings put")) { output = "simulated command failure"; exitCode = 1; }
  return { exitCode, stdout: Buffer.from(output), stderr: Buffer.from("") };
}) as typeof Bun.spawnSync);
const { adb, restoreAndroidState } = await import("../e2e/adb");

beforeEach(() => { commands.length = 0; emulator = "1"; apkPackage = "app.localpass.audit"; failSetting = false; });
afterAll(() => {
  failSetting = false;
  restoreAndroidState();
  run.mockRestore();
  for (const key of keys) {
    if (prior[key] === undefined) delete process.env[key];
    else process.env[key] = prior[key];
  }
});

test("every ADB command selects the explicit serial", () => {
  adb("shell", "getprop", "ro.kernel.qemu");
  expect(commands[0]?.slice(1, 3)).toEqual(["-s", "audit-fixture-serial"]);
});

test("production clear/uninstall and physical-device destruction are refused", () => {
  expect(() => adb("uninstall", "app.localpass")).toThrow();
  expect(() => adb("shell", "pm", "clear", "app.localpass")).toThrow();
  expect(commands).toHaveLength(0);
  emulator = "0";
  expect(() => adb("uninstall", "app.localpass.audit")).toThrow();
  expect(() => adb("shell", "pm", "clear", "app.localpass.audit")).toThrow();
  expect(commands.every(command => command.slice(3).join(" ") === "shell getprop ro.kernel.qemu")).toBe(true);
  expect(() => adb("shell", "locksettings", "clear")).toThrow();
});

test("an APK's manifest must identify the audit package before install", () => {
  apkPackage = "app.localpass";
  expect(() => adb("install", "production.apk")).toThrow();
  expect(commands.some(command => command[3] === "install")).toBe(false);
});

test("cleanup restores original providers and existing port mappings", () => {
  adb("shell", "settings", "put", "secure", "autofill_service", "app.localpass.audit/app.localpass.LpAutofillService");
  adb("forward", "tcp:9333", "localabstract:audit");
  restoreAndroidState();
  expect(commands.map(command => command.slice(3))).toContainEqual(["shell", "settings", "put", "secure", "autofill_service", "app.original/.Service"]);
  expect(commands.map(command => command.slice(3))).toContainEqual(["forward", "tcp:9333", "localabstract:original"]);
  expect(() => adb("forward", "--remove", "tcp:4444")).toThrow();
});

test("failed restoration remains visible and can be retried", () => {
  adb("shell", "settings", "put", "secure", "autofill_service", "audit");
  failSetting = true;
  expect(() => restoreAndroidState()).toThrow("cleanup failed");
  failSetting = false;
  expect(() => restoreAndroidState()).not.toThrow();
});
