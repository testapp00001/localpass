import { useEffect, useId, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";
import { Eye, EyeOff, Loader2, X } from "lucide-react";

type Variant = "primary" | "secondary" | "ghost" | "danger";

const variants: Record<Variant, string> = {
  primary: "bg-indigo-600 text-white hover:bg-indigo-500 disabled:bg-indigo-600/50 shadow-sm",
  secondary:
    "bg-white text-slate-800 ring-1 ring-inset ring-slate-300 hover:bg-slate-50 dark:bg-slate-800 dark:text-slate-100 dark:ring-slate-700 dark:hover:bg-slate-700",
  ghost: "text-slate-700 hover:bg-slate-200/70 dark:text-slate-300 dark:hover:bg-slate-800",
  danger: "bg-rose-600 text-white hover:bg-rose-500 disabled:bg-rose-600/50 shadow-sm",
};

export function Button({
  variant = "secondary",
  busy = false,
  className = "",
  children,
  disabled,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; busy?: boolean }) {
  return (
    <button
      {...rest}
      disabled={disabled || busy}
      className={`inline-flex items-center justify-center gap-2 rounded-lg px-3.5 py-2 text-sm font-medium transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-500 disabled:cursor-not-allowed ${variants[variant]} ${className}`}
    >
      {busy && <Loader2 className="size-4 animate-spin" aria-hidden />}
      {children}
    </button>
  );
}

export function IconButton({
  label,
  className = "",
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      {...rest}
      aria-label={label}
      title={label}
      className={`inline-flex size-9 shrink-0 items-center justify-center rounded-lg text-slate-600 transition-colors hover:bg-slate-200/70 hover:text-slate-900 focus-visible:outline-2 focus-visible:outline-indigo-500 disabled:opacity-40 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100 ${className}`}
    >
      {children}
    </button>
  );
}

const inputClass =
  "w-full rounded-lg border-0 bg-white px-3 py-2 text-sm text-slate-900 ring-1 ring-inset ring-slate-300 placeholder:text-slate-400 focus:ring-2 focus:ring-indigo-500 focus:outline-none dark:bg-slate-900 dark:text-slate-100 dark:ring-slate-700";

export function Field({ label, hint, error, children, htmlFor }: { label: string; hint?: ReactNode; error?: string | null; children: ReactNode; htmlFor?: string }) {
  return (
    <div className="space-y-1.5">
      <label htmlFor={htmlFor} className="block text-sm font-medium text-slate-700 dark:text-slate-300">
        {label}
      </label>
      {children}
      {error ? (
        <p className="text-sm text-rose-600 dark:text-rose-400">{error}</p>
      ) : hint ? (
        <p className="text-xs text-slate-500 dark:text-slate-400">{hint}</p>
      ) : null}
    </div>
  );
}

export function TextInput({ label, hint, error, className = "", ...rest }: InputHTMLAttributes<HTMLInputElement> & { label?: string; hint?: ReactNode; error?: string | null }) {
  const id = useId();
  const input = <input id={id} {...rest} className={`${inputClass} ${className}`} />;
  return label ? (
    <Field label={label} hint={hint} error={error} htmlFor={id}>
      {input}
    </Field>
  ) : (
    input
  );
}

export function PasswordInput({
  label,
  hint,
  error,
  className = "",
  trailing,
  ...rest
}: InputHTMLAttributes<HTMLInputElement> & { label?: string; hint?: ReactNode; error?: string | null; trailing?: ReactNode }) {
  const id = useId();
  const [shown, setShown] = useState(false);
  const input = (
    <div className="relative flex items-center">
      <input
        id={id}
        type={shown ? "text" : "password"}
        autoComplete="off"
        spellCheck={false}
        {...rest}
        className={`${inputClass} secret pr-20 ${className}`}
      />
      <div className="absolute right-1 flex items-center">
        {trailing}
        <IconButton type="button" label={shown ? "Hide" : "Show"} onClick={() => setShown((s) => !s)} className="size-8">
          {shown ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
        </IconButton>
      </div>
    </div>
  );
  return label ? (
    <Field label={label} hint={hint} error={error} htmlFor={id}>
      {input}
    </Field>
  ) : (
    input
  );
}

export function TextArea({ label, className = "", ...rest }: React.TextareaHTMLAttributes<HTMLTextAreaElement> & { label?: string }) {
  const id = useId();
  const area = <textarea id={id} {...rest} className={`${inputClass} min-h-24 ${className}`} />;
  return label ? (
    <Field label={label} htmlFor={id}>
      {area}
    </Field>
  ) : (
    area
  );
}

export function Select({ label, children, className = "", ...rest }: React.SelectHTMLAttributes<HTMLSelectElement> & { label?: string }) {
  const id = useId();
  const select = (
    <select id={id} {...rest} className={`${inputClass} ${className}`}>
      {children}
    </select>
  );
  return label ? (
    <Field label={label} htmlFor={id}>
      {select}
    </Field>
  ) : (
    select
  );
}

export function Toggle({ checked, onChange, label, description }: { checked: boolean; onChange: (v: boolean) => void; label: string; description?: string }) {
  const id = useId();
  return (
    <div className="flex items-start justify-between gap-4 py-2">
      <label htmlFor={id} className="min-w-0 cursor-pointer">
        <span className="block text-sm font-medium text-slate-800 dark:text-slate-200">{label}</span>
        {description && <span className="block text-xs text-slate-500 dark:text-slate-400">{description}</span>}
      </label>
      <button
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`relative mt-0.5 inline-flex h-6 w-11 shrink-0 rounded-full transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-500 ${checked ? "bg-indigo-600" : "bg-slate-300 dark:bg-slate-700"}`}
      >
        <span className={`absolute top-0.5 size-5 rounded-full bg-white shadow transition-transform ${checked ? "translate-x-5.5" : "translate-x-0.5"}`} />
      </button>
    </div>
  );
}

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <div className={`rounded-xl bg-white p-4 shadow-sm ring-1 ring-slate-200 dark:bg-slate-900 dark:ring-slate-800 ${className}`}>
      {children}
    </div>
  );
}

export function Badge({ children, tone = "slate" }: { children: ReactNode; tone?: "slate" | "green" | "amber" | "rose" | "indigo" }) {
  const tones = {
    slate: "bg-slate-100 text-slate-700 dark:bg-slate-800 dark:text-slate-300",
    green: "bg-emerald-100 text-emerald-800 dark:bg-emerald-900/50 dark:text-emerald-300",
    amber: "bg-amber-100 text-amber-800 dark:bg-amber-900/50 dark:text-amber-300",
    rose: "bg-rose-100 text-rose-800 dark:bg-rose-900/50 dark:text-rose-300",
    indigo: "bg-indigo-100 text-indigo-800 dark:bg-indigo-900/50 dark:text-indigo-300",
  };
  return <span className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium ${tones[tone]}`}>{children}</span>;
}

export function Modal({ open, onClose, title, children, footer, wide = false }: { open: boolean; onClose: () => void; title: string; children: ReactNode; footer?: ReactNode; wide?: boolean }) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-40 flex items-end justify-center bg-slate-950/50 p-0 backdrop-blur-sm sm:items-center sm:p-4" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(e) => e.stopPropagation()}
        className={`flex max-h-[92vh] w-full flex-col rounded-t-2xl bg-white shadow-xl ring-1 ring-slate-200 sm:rounded-2xl dark:bg-slate-900 dark:ring-slate-800 ${wide ? "sm:max-w-2xl" : "sm:max-w-md"}`}
      >
        <div className="flex items-center justify-between border-b border-slate-200 px-5 py-3 dark:border-slate-800">
          <h2 className="text-base font-semibold">{title}</h2>
          <IconButton label="Close" onClick={onClose}>
            <X className="size-4" />
          </IconButton>
        </div>
        <div className="overflow-y-auto px-5 py-4">{children}</div>
        {footer && <div className="flex justify-end gap-2 border-t border-slate-200 px-5 py-3 dark:border-slate-800">{footer}</div>}
      </div>
    </div>
  );
}

export function Spinner({ className = "" }: { className?: string }) {
  return <Loader2 className={`size-5 animate-spin text-slate-400 ${className}`} aria-label="Loading" />;
}

export function EmptyState({ icon, title, children }: { icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 px-6 py-16 text-center">
      <div className="rounded-full bg-slate-100 p-4 text-slate-500 dark:bg-slate-800 dark:text-slate-400">{icon}</div>
      <h3 className="text-base font-semibold">{title}</h3>
      {children && <div className="max-w-sm text-sm text-slate-500 dark:text-slate-400">{children}</div>}
    </div>
  );
}

export function ErrorText({ children }: { children: ReactNode }) {
  if (!children) return null;
  return <p className="rounded-lg bg-rose-50 px-3 py-2 text-sm text-rose-700 dark:bg-rose-950/60 dark:text-rose-300">{children}</p>;
}
