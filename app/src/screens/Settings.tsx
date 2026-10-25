import { useEffect, useState } from "react";
import { Download, FileUp, Fingerprint, KeyRound, LogOut, Upload } from "lucide-react";
import { ask, open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorText, type AppInfo, type ImportReport, type Settings as AppSettings } from "../lib/api";
import { Button, Card, ErrorText, Modal, PasswordInput, Select, TextInput, Toggle } from "../components/ui";
import { StrengthMeter } from "../components/StrengthMeter";
import { useToast } from "../components/Toaster";
import { AndroidSetup } from "./AndroidSetup";
import { BrowserExtension } from "./BrowserExtension";
import { WindowsPasskeys } from "./WindowsPasskeys";

const RELEASES_URL = "https://github.com/testapp00001/localpass/releases";

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-2">
      <h2 className="px-1 text-sm font-semibold text-slate-500 uppercase dark:text-slate-400">{title}</h2>
      <Card className="divide-y divide-slate-100 py-1 dark:divide-slate-800">{children}</Card>
    </section>
  );
}

export function Settings({ info, onInfoChange }: { info: AppInfo; onInfoChange: () => void }) {
  const toast = useToast();
  const [s, setS] = useState<AppSettings>(info.settings);
  const [deviceName, setDeviceName] = useState(info.deviceName ?? "");
  const [pwOpen, setPwOpen] = useState(false);
  const [importState, setImportState] = useState<{ path: string; needsPassword: boolean; isKeepass: boolean } | null>(null);
  const [backupOpen, setBackupOpen] = useState(false);
  const desktop = info.platform !== "android";

  useEffect(() => setS(info.settings), [info.settings]);

  const update = async (patch: Partial<AppSettings>) => {
    const next = { ...s, ...patch };
    setS(next);
    try {
      await api.saveSettings(next);
      onInfoChange();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const toggleQuick = async (on: boolean) => {
    try {
      if (on) await api.quickUnlockEnable();
      else await api.quickUnlockDisable();
      toast(on ? "Quick unlock is on" : "Quick unlock is off");
      onInfoChange();
    } catch (e) {
      if (errorText(e) !== "cancelled") toast(errorText(e), "error");
    }
  };

  const pickImport = async () => {
    const path = await openDialog({
      multiple: false,
      filters: [{ name: "Password exports", extensions: ["json", "csv", "kdbx", "lpbackup"] }],
    });
    if (typeof path !== "string") return;
    const lower = path.toLowerCase();
    const isKeepass = lower.endsWith(".kdbx");
    setImportState({ path, isKeepass, needsPassword: isKeepass || lower.endsWith(".lpbackup") });
  };

  const exportJson = async () => {
    const ok = await ask(
      "The exported file contains all your passwords unencrypted. Anyone who gets the file can read them. Continue?",
      { title: "Export unencrypted", kind: "warning" },
    );
    if (!ok) return;
    const path = await saveDialog({ defaultPath: "localpass-export.json", filters: [{ name: "JSON", extensions: ["json"] }] });
    if (!path) return;
    try {
      const leftOut = await api.exportJson(path);
      toast(
        leftOut > 0
          ? `Exported. ${leftOut} passkey${leftOut === 1 ? " is" : "s are"} not in this file (a passkey's private key stays in LocalPass and in the encrypted backup). Delete the file when you're done with it.`
          : "Exported. Delete the file when you're done with it.",
      );
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const leave = async () => {
    const ok = await ask(
      "This deletes the vault from this device only. Your other devices keep their data. If this is your only device, export a backup first.",
      { title: "Remove vault from this device", kind: "warning" },
    );
    if (ok) {
      await api.leaveVault();
      onInfoChange();
    }
  };

  return (
    <div className="mx-auto max-w-2xl space-y-6 p-4 sm:p-6">
      <h1 className="text-xl font-semibold">Settings</h1>

      <Section title="Security">
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <span className="text-sm font-medium">Lock after inactivity</span>
          <Select value={String(s.autoLockMinutes)} onChange={(e) => update({ autoLockMinutes: Number(e.target.value) })} className="w-40">
            {[1, 5, 15, 30, 60].map((m) => (
              <option key={m} value={m}>
                {m} minute{m === 1 ? "" : "s"}
              </option>
            ))}
            <option value={0}>Never</option>
          </Select>
        </div>
        {!desktop && s.autoLockMinutes !== 0 && (
          <p className="px-4 py-2 text-xs text-slate-500 dark:text-slate-400">On a phone, LocalPass also locks one minute after you leave the app.</p>
        )}
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <span className="text-sm font-medium">Clear copied passwords</span>
          <Select value={String(s.clipboardClearSeconds)} onChange={(e) => update({ clipboardClearSeconds: Number(e.target.value) })} className="w-40">
            {[15, 30, 60, 120].map((n) => (
              <option key={n} value={n}>
                after {n} s
              </option>
            ))}
            <option value={0}>Never</option>
          </Select>
        </div>
        {info.quickUnlock.available && (
          <div className="px-4">
            <Toggle
              label={info.quickUnlock.kind === "windowsHello" ? "Unlock with Windows Hello" : "Unlock with fingerprint or face"}
              description="Your master password is still needed if this is turned off or your biometrics change."
              checked={info.quickUnlock.enabled}
              onChange={toggleQuick}
            />
          </div>
        )}
        <div className="flex items-center justify-between px-4 py-3">
          <span className="text-sm font-medium">Master password</span>
          <Button onClick={() => setPwOpen(true)}>
            <KeyRound className="size-4" /> Change
          </Button>
        </div>
      </Section>

      {!desktop && <AndroidSetup />}

      {desktop && <BrowserExtension settings={s} onChange={update} />}

      {desktop && <WindowsPasskeys />}

      {desktop && (
        <Section title="App">
          <div className="px-4">
            <Toggle
              label="Start when I sign in"
              description="Keeps your devices in sync in the background."
              checked={s.startAtLogin}
              onChange={(v) => update({ startAtLogin: v })}
            />
            <Toggle
              label="Keep running in the tray when closed"
              description="Sync continues after you close the window."
              checked={s.closeToTray}
              onChange={(v) => update({ closeToTray: v })}
            />
          </div>
          <div className="flex items-center justify-between gap-4 px-4 py-3">
            <span className="text-sm font-medium">Theme</span>
            <Select value={s.theme} onChange={(e) => update({ theme: e.target.value as AppSettings["theme"] })} className="w-40">
              <option value="system">System</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </Select>
          </div>
        </Section>
      )}

      <Section title="This device">
        <div className="flex items-end gap-2 px-4 py-3">
          <TextInput label="Device name" value={deviceName} onChange={(e) => setDeviceName(e.target.value)} />
          <Button
            disabled={!deviceName.trim() || deviceName === info.deviceName}
            onClick={() =>
              api
                .renameDevice(deviceName)
                .then(() => (toast("Renamed"), onInfoChange()))
                .catch((e) => toast(errorText(e), "error"))
            }
          >
            Save
          </Button>
        </div>
      </Section>

      <Section title="Import & backup">
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div className="text-sm font-medium">Import</div>
            <div className="text-xs text-slate-500">Bitwarden (.json/.csv), KeePass (.kdbx), browser .csv, or a LocalPass backup</div>
          </div>
          <Button onClick={pickImport}>
            <FileUp className="size-4" /> Import…
          </Button>
        </div>
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div className="text-sm font-medium">Encrypted backup</div>
            <div className="text-xs text-slate-500">Protects against losing all your devices</div>
          </div>
          <Button onClick={() => setBackupOpen(true)}>
            <Download className="size-4" /> Back up…
          </Button>
        </div>
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div className="text-sm font-medium">Export unencrypted</div>
            <div className="text-xs text-slate-500">Bitwarden-format .json for moving to another app</div>
          </div>
          <Button variant="ghost" onClick={exportJson}>
            <Upload className="size-4" /> Export…
          </Button>
        </div>
      </Section>

      <Section title="Danger zone">
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <div className="text-sm">Remove the vault from this device</div>
          <Button variant="danger" onClick={leave}>
            <LogOut className="size-4" /> Remove
          </Button>
        </div>
      </Section>

      <p className="pb-4 text-center text-xs text-slate-400">
        LocalPass {info.version} · Everything stays on your devices
        {" · "}
        {/* A plain link: the browser opens it when you click. The app itself never asks the internet for updates. */}
        <button type="button" className="underline hover:text-slate-600 dark:hover:text-slate-300" onClick={() => openUrl(RELEASES_URL)}>
          Releases
        </button>
        {info.quickUnlock.kind === "windowsHello" && !info.quickUnlock.available && (
          <>
            {" · "}
            <Fingerprint className="inline size-3" /> Windows Hello not set up
          </>
        )}
      </p>

      <ChangePasswordModal open={pwOpen} onClose={() => setPwOpen(false)} />
      <ImportModal state={importState} onClose={() => setImportState(null)} />
      <BackupModal open={backupOpen} onClose={() => setBackupOpen(false)} />
    </div>
  );
}

function ChangePasswordModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const toast = useToast();
  const [oldPw, setOld] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [score, setScore] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const reset = () => (setOld(""), setNext(""), setConfirm(""), setError(null), setBusy(false));
  const close = () => (reset(), onClose());
  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.changePassword(oldPw, next);
      toast("Master password changed on this device");
      close();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Modal
      open={open}
      onClose={close}
      title="Change master password"
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} disabled={!oldPw || next.length < 8 || next !== confirm || score < 2} onClick={submit}>
            Change
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <PasswordInput label="Current master password" value={oldPw} onChange={(e) => setOld(e.target.value)} autoFocus />
        <div className="space-y-2">
          <PasswordInput label="New master password" value={next} onChange={(e) => setNext(e.target.value)} />
          <StrengthMeter password={next} onScore={setScore} />
        </div>
        <PasswordInput label="Repeat new master password" value={confirm} onChange={(e) => setConfirm(e.target.value)} error={confirm && confirm !== next ? "Passwords don't match" : null} />
        <p className="text-xs text-slate-500">This changes the password on this device only. Your other devices keep their own.</p>
        <ErrorText>{error}</ErrorText>
      </div>
    </Modal>
  );
}

function ImportModal({ state, onClose }: { state: { path: string; needsPassword: boolean; isKeepass: boolean } | null; onClose: () => void }) {
  const toast = useToast();
  const [password, setPassword] = useState("");
  const [keyfile, setKeyfile] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ImportReport | null>(null);

  useEffect(() => {
    setPassword("");
    setKeyfile(null);
    setError(null);
    setReport(null);
    setBusy(false);
    if (state && !state.needsPassword) run(state.path, null, null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state]);

  const run = async (path: string, pw: string | null, kf: string | null) => {
    setBusy(true);
    setError(null);
    try {
      const r = await api.importFile(path, pw, kf);
      setReport(r);
      window.dispatchEvent(new Event("lp-vault-changed"));
      toast(`Imported ${r.imported} item${r.imported === 1 ? "" : "s"}`);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  if (!state) return null;
  const name = state.path.split(/[\\/]/).pop();
  return (
    <Modal
      open
      onClose={onClose}
      title="Import"
      footer={
        report ? (
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        ) : (
          <>
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            {state.needsPassword && (
              <Button variant="primary" busy={busy} disabled={!password && !keyfile} onClick={() => run(state.path, password || null, keyfile)}>
                Import
              </Button>
            )}
          </>
        )
      }
    >
      <div className="space-y-4">
        <p className="truncate text-sm text-slate-600 dark:text-slate-400">{name}</p>
        {report ? (
          <div className="space-y-2 text-sm">
            <p>
              <b>{report.imported}</b> imported from {report.format}
              {report.duplicates > 0 && <>, {report.duplicates} duplicates skipped</>}.
            </p>
            {report.warnings.length > 0 && (
              <ul className="list-disc space-y-1 pl-5 text-xs text-amber-700 dark:text-amber-300">
                {report.warnings.slice(0, 8).map((w, i) => (
                  <li key={i}>{w}</li>
                ))}
              </ul>
            )}
            {!name?.toLowerCase().endsWith(".lpbackup") && !name?.toLowerCase().endsWith(".kdbx") && (
              <p className="rounded-lg bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:bg-amber-950/50 dark:text-amber-300">
                The file you imported isn't encrypted. Delete it now so nobody else can read your passwords.
              </p>
            )}
          </div>
        ) : state.needsPassword ? (
          <>
            <PasswordInput
              label={state.isKeepass ? "KeePass master password" : "Backup password"}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoFocus
            />
            {state.isKeepass && (
              <div className="flex items-center gap-2 text-sm">
                <Button
                  onClick={async () => {
                    const p = await openDialog({ multiple: false });
                    if (typeof p === "string") setKeyfile(p);
                  }}
                >
                  Key file…
                </Button>
                <span className="truncate text-slate-500">{keyfile ? keyfile.split(/[\\/]/).pop() : "optional"}</span>
              </div>
            )}
          </>
        ) : (
          busy && <p className="text-sm text-slate-500">Importing…</p>
        )}
        <ErrorText>{error}</ErrorText>
      </div>
    </Modal>
  );
}

function BackupModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const toast = useToast();
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const close = () => (setPassword(""), setConfirm(""), setError(null), onClose());
  const submit = async () => {
    const path = await saveDialog({
      defaultPath: `localpass-${new Date().toISOString().slice(0, 10)}.lpbackup`,
      filters: [{ name: "LocalPass backup", extensions: ["lpbackup"] }],
    });
    if (!path) return;
    setBusy(true);
    setError(null);
    try {
      await api.exportBackup(path, password);
      toast("Backup saved");
      close();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      open={open}
      onClose={close}
      title="Encrypted backup"
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} disabled={password.length < 8 || password !== confirm} onClick={submit}>
            Save backup…
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <p className="text-sm text-slate-600 dark:text-slate-400">
          The backup is encrypted with the password you choose here. Restore it from Settings → Import on any device.
        </p>
        <PasswordInput label="Backup password" value={password} onChange={(e) => setPassword(e.target.value)} autoFocus />
        <PasswordInput label="Repeat backup password" value={confirm} onChange={(e) => setConfirm(e.target.value)} error={confirm && confirm !== password ? "Passwords don't match" : null} />
        <ErrorText>{error}</ErrorText>
      </div>
    </Modal>
  );
}
