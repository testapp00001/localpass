import { useEffect, useRef, useState } from "react";
import { Fingerprint, KeyRound, Lock } from "lucide-react";
import { api, errorText, type AppInfo, type UnlockWaiting } from "../lib/api";
import { Button, Card, ErrorText, PasswordInput } from "../components/ui";

export function Unlock({ info, onUnlocked }: { info: AppInfo; onUnlocked: () => void }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const quick = info.quickUnlock;
  // Set once Windows Hello has been offered by itself (or is being offered).
  const triedQuick = useRef(false);
  // The browser waiting for this unlock, if one asked for it. The screen asks on mount and also
  // listens; whichever answer is newer (higher version) wins, so an event that came before the
  // screen was listening is not lost and a slow first answer does not undo a newer event.
  const [waiting, setWaiting] = useState<UnlockWaiting>({ version: -1, name: null });
  useEffect(() => {
    const take = (next: UnlockWaiting) => setWaiting((current) => (next.version > current.version ? next : current));
    const onWaiting = (e: Event) => take((e as CustomEvent<UnlockWaiting>).detail);
    window.addEventListener("lp-unlock-waiting", onWaiting);
    api.brokerUnlockWaiting().then(take).catch(() => {});
    return () => window.removeEventListener("lp-unlock-waiting", onWaiting);
  }, []);

  const unlock = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.unlock(password);
      setPassword("");
      onUnlocked();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  // `auto`: Hello is offered by itself when the screen appears, not because the user pressed the
  // button. The app refuses that ("hidden") while its window is not on screen: then it is running
  // in the tray for a browser or Windows (a passkey request, say) whose own request owns the
  // Hello prompt, and a prompt from here would be a second one. Returns false if refused.
  const unlockQuick = async (auto = false) => {
    setError(null);
    try {
      await api.unlockQuick(auto);
      onUnlocked();
    } catch (e) {
      if (errorText(e) === "hidden") return false;
      // "cancelled" means the user backed out of the prompt: just show the password field.
      if (errorText(e) !== "cancelled") setError(errorText(e));
    }
    return true;
  };

  useEffect(() => {
    // Offer Windows Hello right away when it's set up and the window is on screen. If the
    // window is hidden, wait for the user to open it.
    if (!quick.enabled) return;
    const offer = () => {
      if (triedQuick.current) return;
      triedQuick.current = true;
      unlockQuick(true).then((asked) => {
        if (!asked) triedQuick.current = false;
      });
    };
    window.addEventListener("lp-window-shown", offer);
    offer();
    return () => window.removeEventListener("lp-window-shown", offer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [quick.enabled]);

  return (
    <div className="flex min-h-full items-center justify-center p-4">
      <Card className="w-full max-w-sm p-6">
        <div className="mb-6 space-y-2 text-center">
          <div className="mx-auto flex size-12 items-center justify-center rounded-2xl bg-indigo-600 text-white">
            <Lock className="size-6" />
          </div>
          <h1 className="text-xl font-semibold">Unlock LocalPass</h1>
          {info.deviceName && <p className="text-sm text-slate-500 dark:text-slate-400">{info.deviceName}</p>}
        </div>
        {waiting.name && (
          <p role="status" className="mb-4 rounded-lg bg-indigo-50 px-3 py-2 text-center text-sm text-indigo-900 dark:bg-indigo-950 dark:text-indigo-100">
            {waiting.name} is waiting: unlock LocalPass to continue.
          </p>
        )}
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (password) unlock();
          }}
        >
          <PasswordInput placeholder="Master password" aria-label="Master password" value={password} onChange={(e) => setPassword(e.target.value)} autoFocus />
          <ErrorText>{error}</ErrorText>
          <Button type="submit" variant="primary" className="w-full" disabled={!password} busy={busy}>
            <KeyRound className="size-4" /> Unlock
          </Button>
          {quick.enabled && (
            <Button type="button" className="w-full" onClick={() => unlockQuick()}>
              <Fingerprint className="size-4" /> {quick.kind === "windowsHello" ? "Use Windows Hello" : "Use biometrics"}
            </Button>
          )}
        </form>
        <p className="mt-6 text-center text-xs text-slate-500 dark:text-slate-400">Sync keeps running while LocalPass is locked.</p>
      </Card>
    </div>
  );
}
