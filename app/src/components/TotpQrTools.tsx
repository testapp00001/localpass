// Desktop ways to fill the 2FA field from a QR code: a pasted screenshot (the main path), the
// screen, and a picture file (chosen or dropped). A phone has the camera button instead.
// The picture is decoded in Rust; only the setup link comes back to this page.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { CheckCircle2, ClipboardPaste, ImageUp, Monitor } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api, errorText, type TotpQr } from "../lib/api";
import { Button } from "./ui";
import { useToast } from "./Toaster";

type Source = "clipboard" | "screen" | "file";

function describe(found: TotpQr): string {
  const who = [found.issuer, found.account && `(${found.account})`].filter(Boolean).join(" ");
  return who ? `Found: ${who}` : "Found a two-factor setup code";
}

/**
 * Wraps the 2FA field. `value` is the field's current text: the "Found" line only stays while it
 * still holds what the QR code gave.
 */
export function TotpQrTools({ value, onFound, children }: { value: string; onFound: (uri: string) => void; children: ReactNode }) {
  const toast = useToast();
  const box = useRef<HTMLDivElement>(null);
  const [busy, setBusy] = useState<Source | null>(null);
  const [dragging, setDragging] = useState(false);
  const [found, setFound] = useState<{ uri: string; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The latest callbacks, so the listeners below can be set up once.
  const run = useRef<(source: Source, job: () => Promise<TotpQr | null>) => void>(() => {});
  run.current = (source, job) => {
    if (busy) return;
    setBusy(source);
    setError(null);
    job()
      .then((r) => {
        if (!r) return; // the user closed the screen picker
        onFound(r.uri);
        setFound({ uri: r.uri, text: describe(r) });
        if (r.clipboardCleared) toast("Picture removed from the clipboard");
      })
      .catch((e) => (setFound(null), setError(errorText(e))))
      .finally(() => setBusy(null));
  };

  const pickFile = async () => {
    const path = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Pictures", extensions: ["png", "jpg", "jpeg", "bmp", "webp"] }],
    }).catch(() => null);
    if (typeof path === "string") run.current("file", () => api.totpQrFromFile(path));
  };

  // Ctrl+V while focus is in the 2FA block. Text still pastes as usual; only a paste with no text
  // (a copied picture) goes to Rust, which reads the picture itself.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if (!box.current?.contains(document.activeElement)) return;
      if (e.clipboardData?.getData("text/plain")) return;
      e.preventDefault();
      run.current("clipboard", () => api.totpQrFromClipboard());
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, []);

  // A picture file dropped on the block (the web view gives paths, not data).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let gone = false;
    const inside = (p: { x: number; y: number }) => {
      const r = box.current?.getBoundingClientRect();
      const x = p.x / window.devicePixelRatio;
      const y = p.y / window.devicePixelRatio;
      return !!r && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
    };
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const e = event.payload;
        if (e.type === "leave") setDragging(false);
        else if (e.type === "enter" || e.type === "over") setDragging(inside(e.position));
        else if (e.type === "drop") {
          setDragging(false);
          if (e.paths.length && inside(e.position)) run.current("file", () => api.totpQrFromFile(e.paths[0]));
        }
      })
      .then((u) => (gone ? u() : (unlisten = u)))
      .catch(() => {});
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);

  const line = found && found.uri === value ? found.text : null;
  return (
    <div ref={box} className={`space-y-2 rounded-xl ${dragging ? "ring-2 ring-indigo-400 ring-offset-4 dark:ring-offset-slate-950" : ""}`}>
      {children}
      <div className="flex flex-wrap gap-2">
        <Button type="button" busy={busy === "clipboard"} disabled={!!busy} onClick={() => run.current("clipboard", () => api.totpQrFromClipboard())}>
          <ClipboardPaste className="size-4" />
          Paste QR image
        </Button>
        <Button type="button" busy={busy === "screen"} disabled={!!busy} onClick={() => run.current("screen", () => api.totpQrFromScreen())}>
          <Monitor className="size-4" />
          Scan QR on screen
        </Button>
        <Button type="button" busy={busy === "file"} disabled={!!busy} onClick={pickFile}>
          <ImageUp className="size-4" />
          From image file…
        </Button>
      </div>
      <p className="text-xs text-slate-500 dark:text-slate-400">
        Copy a screenshot of the QR code (PrtSc, Win+Shift+S, or “Copy” in your browser’s screenshot tool), then press Ctrl+V here. “Scan QR on
        screen” makes LocalPass step aside for a moment. A picture file can be dropped here too. Nothing is saved to disk.
      </p>
      <div aria-live="polite">
        {line && (
          <p className="flex items-center gap-1.5 text-sm text-emerald-700 dark:text-emerald-400">
            <CheckCircle2 className="size-4 shrink-0" /> {line}
          </p>
        )}
        {error && <p className="text-sm text-rose-600 dark:text-rose-400">{error}</p>}
      </div>
    </div>
  );
}
