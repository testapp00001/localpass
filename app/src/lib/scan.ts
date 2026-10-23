// Camera QR scanning (Android). The scanner draws the camera behind the web view,
// so while it runs the page goes see-through and <ScanOverlay> shows the controls.

import { useSyncExternalStore } from "react";
import { onBackButtonPress } from "@tauri-apps/api/app";
import { Format, cancel, checkPermissions, requestPermissions, scan } from "@tauri-apps/plugin-barcode-scanner";

/** Camera scanning exists on phones only. */
export const canScan = /Android/i.test(navigator.userAgent);

let scanning = false;
const listeners = new Set<() => void>();

function setScanning(value: boolean) {
  scanning = value;
  document.documentElement.classList.toggle("scanning", value);
  listeners.forEach((l) => l());
}

export function useScanning(): boolean {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => scanning,
  );
}

// The plugin's `cancel` tears the camera down and then tries to reject the pending `scan()`
// call, but it has already forgotten that call by then (tauri-plugin-barcode-scanner 2.5.2,
// `cancel` runs `destroy()` before `savedInvoke?.reject`). The promise never settles, so
// Cancel and the Back button left the see-through scanning screen on top. Settle it here.
let endScan: (() => void) | null = null;

export function cancelScan() {
  cancel().catch(() => {});
  endScan?.();
}

/** Opens the camera and resolves with the first QR code's text, or null if the user backs out. */
export async function scanQr(): Promise<string | null> {
  let permission = await checkPermissions();
  if (permission !== "granted") permission = await requestPermissions();
  if (permission !== "granted") {
    throw new Error("Camera access is off for LocalPass. Turn it on in your phone's app settings to scan codes.");
  }
  // While scanning, the system Back button closes the camera instead of leaving the app.
  const back = await onBackButtonPress(() => cancelScan()).catch(() => null);
  const cancelled = new Promise<null>((resolve) => {
    endScan = () => resolve(null);
  });
  setScanning(true);
  try {
    const pending = scan({ formats: [Format.QRCode], windowed: true }).then((r) => r.content);
    pending.catch(() => {}); // a scan we stopped waiting for may still reject later
    return await Promise.race([pending, cancelled]);
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    if (/cancel/i.test(message)) return null;
    throw new Error(`Could not scan: ${message}`);
  } finally {
    endScan = null;
    setScanning(false);
    back?.unregister().catch(() => {});
  }
}
