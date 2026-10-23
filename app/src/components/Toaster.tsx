import { createContext, useCallback, useContext, useState, type ReactNode } from "react";
import { CheckCircle2, AlertTriangle, Info } from "lucide-react";

type Tone = "success" | "error" | "info";
interface Toast {
  id: number;
  tone: Tone;
  text: string;
}

const ToastContext = createContext<(text: string, tone?: Tone) => void>(() => {});

export function useToast() {
  return useContext(ToastContext);
}

let nextId = 1;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((text: string, tone: Tone = "success") => {
    const id = nextId++;
    setToasts((t) => [...t.slice(-3), { id, tone, text }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), tone === "error" ? 6000 : 2800);
  }, []);
  const icon = { success: CheckCircle2, error: AlertTriangle, info: Info };
  return (
    <ToastContext.Provider value={push}>
      {children}
      <div className="pointer-events-none fixed inset-x-0 bottom-4 z-50 flex flex-col items-center gap-2 px-4" aria-live="polite">
        {toasts.map((t) => {
          const Icon = icon[t.tone];
          return (
            <div
              key={t.id}
              className={`pointer-events-auto flex max-w-md items-center gap-2 rounded-xl px-4 py-2.5 text-sm shadow-lg ring-1 ${
                t.tone === "error"
                  ? "bg-rose-600 text-white ring-rose-700"
                  : "bg-slate-900 text-white ring-slate-800 dark:bg-slate-100 dark:text-slate-900 dark:ring-slate-200"
              }`}
            >
              <Icon className="size-4 shrink-0" aria-hidden />
              <span>{t.text}</span>
            </div>
          );
        })}
      </div>
    </ToastContext.Provider>
  );
}
