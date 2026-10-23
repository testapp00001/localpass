import { useEffect, useState } from "react";
import { api, type Strength } from "../lib/api";

const labels = ["Very weak", "Weak", "Fair", "Strong", "Very strong"];
const colors = ["bg-rose-500", "bg-orange-500", "bg-amber-400", "bg-emerald-500", "bg-emerald-600"];

/** zxcvbn-based strength estimate (computed in Rust). */
export function StrengthMeter({ password, onScore }: { password: string; onScore?: (score: number) => void }) {
  const [s, setS] = useState<Strength | null>(null);
  useEffect(() => {
    if (!password) {
      setS(null);
      onScore?.(0);
      return;
    }
    const t = setTimeout(() => {
      api.passwordStrength(password).then((r) => {
        setS(r);
        onScore?.(r.score);
      });
    }, 150);
    return () => clearTimeout(t);
  }, [password, onScore]);
  const score = s?.score ?? -1;
  return (
    <div className="space-y-1.5" aria-live="polite">
      <div className="flex gap-1">
        {[0, 1, 2, 3, 4].map((i) => (
          <div key={i} className={`h-1.5 flex-1 rounded-full ${i <= score ? colors[score] : "bg-slate-200 dark:bg-slate-800"}`} />
        ))}
      </div>
      {s && (
        <p className="text-xs text-slate-500 dark:text-slate-400">
          <span className="font-medium text-slate-700 dark:text-slate-300">{labels[s.score]}</span>
          {" · "}
          {s.crackTime} to crack offline
          {s.warning ? ` · ${s.warning}` : ""}
        </p>
      )}
    </div>
  );
}
