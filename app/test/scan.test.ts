// Host-side test of src/lib/scan.ts with a fake barcode plugin that behaves like tauri-plugin-barcode-scanner
// 2.5.2 on Android: `cancel()` forgets the pending `scan()` call without settling it. Run: cd app && bun test
// (This folder is outside tsconfig's "include", so `bun run build` does not type-check it.)

import { beforeEach, describe, expect, mock, test } from "bun:test";

let pendingScan: { resolve: (r: { content: string }) => void; reject: (e: Error) => void } | null = null;
let cancelCalls = 0;
let backHandler: (() => void) | null = null;
let backUnregistered = 0;
let permission = "granted";
const classes = new Set<string>();

(globalThis as any).document = { documentElement: { classList: { toggle: (c: string, on: boolean) => (on ? classes.add(c) : classes.delete(c)) } } };
(globalThis as any).navigator = { userAgent: "Android" };

mock.module("@tauri-apps/plugin-barcode-scanner", () => ({
  Format: { QRCode: "QR_CODE" },
  checkPermissions: async () => permission,
  requestPermissions: async () => permission,
  scan: () => new Promise((resolve, reject) => (pendingScan = { resolve, reject })),
  // The bug: the plugin drops the pending call, then tries to reject nothing.
  cancel: async () => {
    cancelCalls++;
    pendingScan = null;
  },
}));
mock.module("@tauri-apps/api/app", () => ({
  onBackButtonPress: async (cb: () => void) => {
    backHandler = cb;
    return { unregister: async () => void backUnregistered++ };
  },
}));

const { scanQr, cancelScan } = await import("../src/lib/scan");

beforeEach(() => {
  pendingScan = null;
  cancelCalls = 0;
  backHandler = null;
  backUnregistered = 0;
  permission = "granted";
  classes.clear();
});
const tick = () => new Promise((r) => setTimeout(r, 5));

describe("scanQr", () => {
  test("returns the scanned text and leaves the scanning state", async () => {
    const result = scanQr();
    await tick();
    expect(classes.has("scanning")).toBe(true);
    pendingScan!.resolve({ content: "otpauth://totp/x?secret=ABC" });
    expect(await result).toBe("otpauth://totp/x?secret=ABC");
    expect(classes.has("scanning")).toBe(false);
    expect(backUnregistered).toBe(1);
  });

  test("the Cancel button ends the scan although the plugin never settles it", async () => {
    const result = scanQr();
    await tick();
    cancelScan();
    expect(await result).toBeNull();
    expect(cancelCalls).toBe(1);
    expect(classes.has("scanning")).toBe(false);
    expect(backUnregistered).toBe(1);
  });

  test("the system Back button ends the scan too", async () => {
    const result = scanQr();
    await tick();
    backHandler!();
    expect(await result).toBeNull();
    expect(classes.has("scanning")).toBe(false);
    expect(backUnregistered).toBe(1);
  });

  test("the scanner can be opened again after a cancel, and a late scan result is ignored", async () => {
    const first = scanQr();
    await tick();
    const stale = pendingScan!;
    cancelScan();
    expect(await first).toBeNull();
    const second = scanQr();
    await tick();
    expect(classes.has("scanning")).toBe(true);
    stale.resolve({ content: "late" }); // the first call's result arrives after it was cancelled
    pendingScan!.resolve({ content: "fresh" });
    expect(await second).toBe("fresh");
    expect(classes.has("scanning")).toBe(false);
  });

  test("Cancel with no scan running does nothing", () => {
    cancelScan();
    expect(classes.has("scanning")).toBe(false);
  });

  test("a plugin that does reject on cancel still gives null", async () => {
    const result = scanQr();
    await tick();
    pendingScan!.reject(new Error("cancelled"));
    expect(await result).toBeNull();
    expect(classes.has("scanning")).toBe(false);
  });

  test("another scan error is reported and the scanning state is left", async () => {
    const result = scanQr();
    await tick();
    pendingScan!.reject(new Error("camera failed"));
    expect(result).rejects.toThrow("Could not scan: camera failed");
    await tick();
    expect(classes.has("scanning")).toBe(false);
  });

  test("without camera permission it throws before showing the scanning screen", async () => {
    permission = "denied";
    await expect(scanQr()).rejects.toThrow("Camera access is off");
    expect(classes.has("scanning")).toBe(false);
  });
});
