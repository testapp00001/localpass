import { useEffect, useState } from "react";
import { Copy } from "lucide-react";
import { api, type TotpCode as Code } from "../lib/api";
import { IconButton } from "./ui";
import { useToast } from "./Toaster";

/** Live 2FA code with a countdown ring. */
export function TotpCode({ itemId }: { itemId: string }) {
  const toast = useToast();
  const [code, setCode] = useState<Code | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let alive = true;
    const tick = () =>
      api
        .totpCode(itemId)
        .then((c) => alive && (setCode(c), setError(false)))
        .catch(() => alive && setError(true));
    tick();
    const t = setInterval(tick, 1000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [itemId]);

  if (error) return <span className="text-sm text-rose-600">Invalid 2FA secret</span>;
  if (!code) return null;
  const pct = code.remaining / code.period;
  const r = 9;
  const c = 2 * Math.PI * r;
  const formatted = code.code.length === 6 ? `${code.code.slice(0, 3)} ${code.code.slice(3)}` : code.code;
  return (
    <div className="flex items-center gap-3">
      <span className="secret text-xl font-semibold tabular-nums tracking-wider">{formatted}</span>
      <svg viewBox="0 0 24 24" className="size-6 -rotate-90" aria-label={`${code.remaining} seconds left`}>
        <circle cx="12" cy="12" r={r} className="fill-none stroke-slate-200 dark:stroke-slate-700" strokeWidth="3" />
        <circle
          cx="12"
          cy="12"
          r={r}
          className={`fill-none transition-[stroke-dashoffset] duration-1000 ease-linear ${code.remaining <= 5 ? "stroke-rose-500" : "stroke-indigo-500"}`}
          strokeWidth="3"
          strokeDasharray={c}
          strokeDashoffset={c * (1 - pct)}
          strokeLinecap="round"
        />
      </svg>
      <span className="text-xs tabular-nums text-slate-500">{code.remaining}s</span>
      <IconButton label="Copy code" onClick={() => api.copyField(itemId, "totp").then(() => toast("Code copied"))}>
        <Copy className="size-4" />
      </IconButton>
    </div>
  );
}
