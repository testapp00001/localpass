import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { cancelScan, useScanning } from "../lib/scan";

/** Controls shown over the camera picture while a QR code is being scanned. */
export function ScanOverlay() {
  const scanning = useScanning();
  if (!scanning) return null;
  return createPortal(
    <div className="fixed inset-0 z-[100] flex flex-col items-center justify-between gap-6 p-6 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(1.5rem,env(safe-area-inset-bottom))]">
      <p className="rounded-full bg-black/65 px-4 py-2 text-center text-sm text-white">Point the camera at the QR code</p>
      <div className="aspect-square w-3/4 max-w-72 rounded-3xl border-2 border-white/90 shadow-[0_0_0_9999px_rgba(0,0,0,0.35)]" aria-hidden />
      <button
        type="button"
        onClick={cancelScan}
        className="inline-flex items-center gap-2 rounded-full bg-white px-5 py-2.5 text-sm font-medium text-slate-900 shadow-lg"
      >
        <X className="size-4" />
        Cancel
      </button>
    </div>,
    document.body,
  );
}
