import { useState } from "react";
import { ArrowLeft, Check, KeyRound, Laptop, ScanLine, ShieldCheck, Smartphone, Wifi } from "lucide-react";
import { api, errorText, type AppInfo } from "../lib/api";
import { Button, Card, ErrorText, PasswordInput, TextInput, Toggle } from "../components/ui";
import { StrengthMeter } from "../components/StrengthMeter";
import { canScan, scanQr } from "../lib/scan";

type Step = "welcome" | "create" | "join";

export function Onboarding({ info, onDone }: { info: AppInfo; onDone: () => void }) {
  const [step, setStep] = useState<Step>("welcome");
  return (
    <div className="flex min-h-full items-center justify-center p-4 sm:p-8">
      <div className="w-full max-w-lg">
        {step === "welcome" && <Welcome onCreate={() => setStep("create")} onJoin={() => setStep("join")} />}
        {step === "create" && <CreateVault info={info} onBack={() => setStep("welcome")} onDone={onDone} />}
        {step === "join" && <JoinVault info={info} onBack={() => setStep("welcome")} onDone={onDone} />}
      </div>
    </div>
  );
}

function Welcome({ onCreate, onJoin }: { onCreate: () => void; onJoin: () => void }) {
  return (
    <div className="space-y-6">
      <div className="space-y-3 text-center">
        <div className="mx-auto flex size-14 items-center justify-center rounded-2xl bg-indigo-600 text-white shadow-lg shadow-indigo-600/30">
          <KeyRound className="size-7" />
        </div>
        <h1 className="text-2xl font-semibold tracking-tight">LocalPass</h1>
        <p className="text-slate-600 dark:text-slate-400">
          Your passwords never leave your devices. They sync directly between your phone and computers whenever they're on the same network.
        </p>
      </div>
      <div className="grid gap-3">
        <button
          onClick={onCreate}
          className="flex items-start gap-4 rounded-xl bg-white p-4 text-left shadow-sm ring-1 ring-slate-200 transition hover:ring-indigo-400 dark:bg-slate-900 dark:ring-slate-800 dark:hover:ring-indigo-500"
        >
          <ShieldCheck className="mt-0.5 size-6 shrink-0 text-indigo-600" />
          <span>
            <span className="block font-medium">Create a new vault</span>
            <span className="block text-sm text-slate-500 dark:text-slate-400">Start fresh on this device. You can add your other devices afterwards.</span>
          </span>
        </button>
        <button
          onClick={onJoin}
          className="flex items-start gap-4 rounded-xl bg-white p-4 text-left shadow-sm ring-1 ring-slate-200 transition hover:ring-indigo-400 dark:bg-slate-900 dark:ring-slate-800 dark:hover:ring-indigo-500"
        >
          <span className="mt-0.5 flex shrink-0 text-indigo-600">
            <Smartphone className="size-6" />
          </span>
          <span>
            <span className="block font-medium">I already use LocalPass on another device</span>
            <span className="block text-sm text-slate-500 dark:text-slate-400">Join its vault with a pairing code. Both devices need to be on the same network.</span>
          </span>
        </button>
      </div>
    </div>
  );
}

function Header({ title, onBack }: { title: string; onBack: () => void }) {
  return (
    <div className="mb-4 flex items-center gap-2">
      <button onClick={onBack} className="rounded-lg p-1.5 text-slate-500 hover:bg-slate-200/70 dark:hover:bg-slate-800" aria-label="Back">
        <ArrowLeft className="size-5" />
      </button>
      <h1 className="text-xl font-semibold">{title}</h1>
    </div>
  );
}

function PasswordFields({
  password,
  setPassword,
  confirm,
  setConfirm,
  onScore,
}: {
  password: string;
  setPassword: (v: string) => void;
  confirm: string;
  setConfirm: (v: string) => void;
  onScore: (s: number) => void;
}) {
  return (
    <>
      <div className="space-y-2">
        <PasswordInput
          label="Master password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          autoFocus
          hint="A long passphrase works best, e.g. four random words."
        />
        <StrengthMeter password={password} onScore={onScore} />
      </div>
      <PasswordInput
        label="Repeat master password"
        value={confirm}
        onChange={(e) => setConfirm(e.target.value)}
        error={confirm && confirm !== password ? "Passwords don't match" : null}
      />
      <p className="rounded-lg bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:bg-amber-950/50 dark:text-amber-300">
        There is no recovery if you forget it: nothing is stored in a cloud. Write it down and keep it somewhere safe.
      </p>
    </>
  );
}

function AutostartToggle({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <Toggle
      label="Sync in the background"
      description="Start LocalPass when you sign in so your devices stay in sync."
      checked={checked}
      onChange={onChange}
    />
  );
}

async function applyAutostart(info: AppInfo, enabled: boolean) {
  if (info.platform === "android") return;
  try {
    await api.saveSettings({ ...info.settings, startAtLogin: enabled });
  } catch {
    // Not fatal: the user can change it in Settings.
  }
}

function CreateVault({ info, onBack, onDone }: { info: AppInfo; onBack: () => void; onDone: () => void }) {
  const [name, setName] = useState(info.suggestedDeviceName);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [score, setScore] = useState(0);
  const [autostart, setAutostart] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const valid = name.trim() && password.length >= 8 && password === confirm && score >= 2;

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.createVault(password, name.trim());
      await applyAutostart(info, autostart);
      onDone();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Card className="p-6">
      <Header title="Create your vault" onBack={onBack} />
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) submit();
        }}
      >
        <TextInput label="Name of this device" value={name} onChange={(e) => setName(e.target.value)} hint="Shown on your other devices, e.g. “Home laptop”." />
        <PasswordFields password={password} setPassword={setPassword} confirm={confirm} setConfirm={setConfirm} onScore={setScore} />
        {info.platform !== "android" && <AutostartToggle checked={autostart} onChange={setAutostart} />}
        <ErrorText>{error}</ErrorText>
        <Button type="submit" variant="primary" className="w-full" disabled={!valid} busy={busy}>
          {busy ? "Creating vault…" : "Create vault"}
        </Button>
        {password && score < 2 && <p className="text-center text-xs text-slate-500">Choose a stronger master password to continue.</p>}
      </form>
    </Card>
  );
}

function JoinVault({ info, onBack, onDone }: { info: AppInfo; onBack: () => void; onDone: () => void }) {
  const [code, setCode] = useState("");
  const [addr, setAddr] = useState("");
  const [showAddr, setShowAddr] = useState(false);
  const [name, setName] = useState(info.suggestedDeviceName);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [score, setScore] = useState(0);
  const [autostart, setAutostart] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const digits = code.replace(/\D/g, "");
  const isQr = code.trim().startsWith("localpass://");
  const valid = (digits.length === 8 || isQr) && name.trim() && password.length >= 8 && password === confirm && score >= 2;

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const addrs = addr.split(/[\s,]+/).filter(Boolean);
      await api.joinVault(isQr ? code.trim() : digits, addrs, password, name.trim());
      await applyAutostart(info, autostart);
      onDone();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  const scanCode = async () => {
    setError(null);
    try {
      const text = (await scanQr())?.trim();
      if (!text) return;
      if (text.startsWith("localpass://")) setCode(text);
      else setError("That is not a LocalPass pairing code. Open Devices → Add device on your other device.");
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <Card className="p-6">
      <Header title="Join your vault" onBack={onBack} />
      <ol className="mb-5 space-y-2 text-sm text-slate-600 dark:text-slate-400">
        <li className="flex gap-2">
          <Laptop className="mt-0.5 size-4 shrink-0 text-indigo-600" /> On a device that already has your vault, open <b>Devices → Add device</b>.
        </li>
        <li className="flex gap-2">
          <Wifi className="mt-0.5 size-4 shrink-0 text-indigo-600" /> Keep both devices on the same Wi-Fi or network, then enter the code shown there.
        </li>
      </ol>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) submit();
        }}
      >
        {isQr ? (
          <div className="flex items-center gap-3 rounded-lg bg-emerald-50 px-3 py-2.5 text-sm text-emerald-800 dark:bg-emerald-950/40 dark:text-emerald-300">
            <Check className="size-4 shrink-0" />
            <span className="flex-1">QR code scanned</span>
            <button type="button" className="font-medium underline" onClick={() => setCode("")}>
              Clear
            </button>
          </div>
        ) : (
          <div className="space-y-2">
            <TextInput
              label="Pairing code"
              value={code}
              onChange={(e) => setCode(e.target.value)}
              placeholder="1234-5678"
              inputMode="numeric"
              autoFocus
              className="text-center text-lg tracking-widest"
            />
            {canScan && (
              <Button type="button" variant="secondary" className="w-full" onClick={scanCode}>
                <ScanLine className="size-4" />
                Scan QR code instead
              </Button>
            )}
          </div>
        )}
        {showAddr ? (
          <TextInput
            label="Address of the other device (optional)"
            value={addr}
            onChange={(e) => setAddr(e.target.value)}
            placeholder="192.168.1.20:47821"
            hint="Shown under the code on the other device. Needed only if networks block discovery."
          />
        ) : (
          <button type="button" className="text-sm text-indigo-600 hover:underline dark:text-indigo-400" onClick={() => setShowAddr(true)}>
            Can't find the other device?
          </button>
        )}
        <TextInput label="Name of this device" value={name} onChange={(e) => setName(e.target.value)} />
        <PasswordFields password={password} setPassword={setPassword} confirm={confirm} setConfirm={setConfirm} onScore={setScore} />
        <p className="text-xs text-slate-500 dark:text-slate-400">Each device has its own master password. Using the same one everywhere is easiest.</p>
        {info.platform !== "android" && <AutostartToggle checked={autostart} onChange={setAutostart} />}
        <ErrorText>{error}</ErrorText>
        <Button type="submit" variant="primary" className="w-full" disabled={!valid} busy={busy}>
          {busy ? "Waiting for approval on the other device…" : "Join vault"}
        </Button>
      </form>
    </Card>
  );
}
