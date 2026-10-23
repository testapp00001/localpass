import { useCallback, useEffect, useState } from "react";
import { Copy, RefreshCw } from "lucide-react";
import { api, errorText, type PasswordOptions } from "../lib/api";
import { Button, IconButton, Modal, Toggle } from "./ui";
import { useToast } from "./Toaster";

const defaults: PasswordOptions = { length: 20, lowercase: true, uppercase: true, digits: true, symbols: true, avoidAmbiguous: false };

export function PasswordGenerator({ open, onClose, onUse }: { open: boolean; onClose: () => void; onUse?: (password: string) => void }) {
  const toast = useToast();
  const [opts, setOpts] = useState<PasswordOptions>(defaults);
  const [value, setValue] = useState("");
  const [bits, setBits] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const regenerate = useCallback(() => {
    api
      .generatePassword(opts)
      .then((r) => {
        setValue(r.password);
        setBits(r.entropyBits);
        setError(null);
      })
      .catch((e) => setError(errorText(e)));
  }, [opts]);

  useEffect(() => {
    if (open) regenerate();
  }, [open, regenerate]);

  const set = (patch: Partial<PasswordOptions>) => setOpts((o) => ({ ...o, ...patch }));

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Generate password"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
          {onUse && (
            <Button variant="primary" disabled={!value} onClick={() => (onUse(value), onClose())}>
              Use password
            </Button>
          )}
        </>
      }
    >
      <div className="space-y-4">
        <div className="flex items-center gap-1 rounded-lg bg-slate-100 p-2 dark:bg-slate-800">
          <code className="secret min-w-0 flex-1 break-all px-1 text-base">{value || "…"}</code>
          <IconButton label="Generate another" onClick={regenerate}>
            <RefreshCw className="size-4" />
          </IconButton>
          <IconButton
            label="Copy"
            disabled={!value}
            onClick={() => api.copyText(value).then((s) => toast(s ? `Copied · clears in ${s}s` : "Copied"))}
          >
            <Copy className="size-4" />
          </IconButton>
        </div>
        {error ? <p className="text-sm text-rose-600">{error}</p> : <p className="text-xs text-slate-500">{Math.round(bits)} bits of entropy</p>}
        <div>
          <label className="flex items-center justify-between text-sm font-medium">
            Length <span className="tabular-nums text-slate-500">{opts.length}</span>
          </label>
          <input
            type="range"
            min={8}
            max={64}
            value={opts.length}
            onChange={(e) => set({ length: Number(e.target.value) })}
            className="mt-2 w-full accent-indigo-600"
          />
        </div>
        <div className="divide-y divide-slate-200 dark:divide-slate-800">
          <Toggle label="Lowercase (a–z)" checked={opts.lowercase} onChange={(v) => set({ lowercase: v })} />
          <Toggle label="Uppercase (A–Z)" checked={opts.uppercase} onChange={(v) => set({ uppercase: v })} />
          <Toggle label="Digits (0–9)" checked={opts.digits} onChange={(v) => set({ digits: v })} />
          <Toggle label="Symbols (!@#…)" checked={opts.symbols} onChange={(v) => set({ symbols: v })} />
          <Toggle label="Avoid look-alikes" description="Skips characters like I, l, 1, O and 0" checked={opts.avoidAmbiguous} onChange={(v) => set({ avoidAmbiguous: v })} />
        </div>
      </div>
    </Modal>
  );
}
