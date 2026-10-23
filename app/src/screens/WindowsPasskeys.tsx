import { useCallback, useEffect, useRef, useState } from "react";
import { ExternalLink, Link2, Trash2 } from "lucide-react";
import { api, errorText, type WindowsPasskeysStatus as Status } from "../lib/api";
import { Badge, Button, Card, Modal } from "../components/ui";
import { useToast } from "../components/Toaster";

/** What the user has to do next. */
type Step = "unsupported" | "files" | "setup" | "enable" | "pair" | "done";

function nextStep(s: Status): Step {
  if (!s.supported) return "unsupported";
  if (!s.providerExeFound && !s.packageRegistered) return "files";
  // A set-up from another copy of LocalPass (a development build, say) works as it is.
  if (!s.packageRegistered || !s.registeredInWindows) return "setup";
  if (!s.enabledInWindows) return "enable";
  if (!s.paired) return "pair";
  return "done";
}

/**
 * Settings section for the Windows passkey provider (Windows only): LocalPass becomes an option in the
 * system's passkey sheet for every app and browser. The app registers the provider and pairs with it;
 * turning it on is the user's own Windows Hello check in Windows settings, which no app can do.
 */
export function WindowsPasskeys() {
  const toast = useToast();
  const [status, setStatus] = useState<Status | null>(null);
  const [busy, setBusy] = useState<"setup" | "pair" | "remove" | null>(null);
  const [removeOpen, setRemoveOpen] = useState(false);
  const [forget, setForget] = useState(false);
  const reading = useRef(false);

  const reload = useCallback(() => {
    // Reading the status starts a few programs: one at a time.
    if (reading.current) return;
    reading.current = true;
    api
      .windowsPasskeysStatus()
      .then(setStatus)
      .catch(() => {})
      .finally(() => {
        reading.current = false;
      });
  }, []);

  useEffect(() => {
    reload();
    const onVisible = () => document.visibilityState === "visible" && reload();
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", reload);
    window.addEventListener("lp-browser-paired", reload);
    return () => {
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", reload);
      window.removeEventListener("lp-browser-paired", reload);
    };
  }, [reload]);

  if (!status?.available) return null;

  const step = nextStep(status);
  const added = status.packageRegistered && status.registeredInWindows;
  const otherCopy = added && !status.packageInBundle;
  const canRepair = added && status.providerExeFound && (otherCopy || !status.appPathOk);

  const run = async (what: "setup" | "pair" | "remove", action: () => Promise<Status>, done?: string) => {
    setBusy(what);
    try {
      setStatus(await action());
      if (done) toast(done);
    } catch (e) {
      toast(errorText(e), "error");
      reload();
    } finally {
      setBusy(null);
    }
  };

  const openWindowsSettings = () => api.windowsPasskeysOpenSettings().catch((e) => toast(errorText(e), "error"));

  return (
    <section className="space-y-2">
      <h2 className="px-1 text-sm font-semibold text-slate-500 uppercase dark:text-slate-400">Windows passkeys</h2>
      <Card className="divide-y divide-slate-100 py-1 dark:divide-slate-800">
        <div className="space-y-1.5 px-4 py-3 text-xs text-slate-500 dark:text-slate-400">
          <p>
            Makes LocalPass one of the choices in Windows' passkey sheet, so any app and any browser can save passkeys in LocalPass and sign in with them, not only Chrome and Edge with the LocalPass extension.
          </p>
          <p>If you use the extension too, turn off its passkey switch in the extension popup, or the browser asks twice.</p>
        </div>

        {step === "unsupported" && (
          <div className="px-4 py-3 text-sm text-slate-600 dark:text-slate-300">
            This needs Windows 11 24H2 with the September 2025 update or later (build 26100.6725).
            {status.windowsVersion && <span className="text-slate-500 dark:text-slate-400"> This PC has build {status.windowsVersion}.</span>}
          </div>
        )}

        {step === "files" && (
          <div className="px-4 py-3 text-sm text-amber-700 dark:text-amber-400">
            The passkey provider is not part of this copy of LocalPass. Install LocalPass with its installer.
          </div>
        )}

        {step !== "unsupported" && step !== "files" && (
          <>
            <StatusRow
              title="Added to Windows"
              on={added}
              text={
                added
                  ? otherCopy
                    ? "LocalPass is in Windows' list of passkey providers, set up from another copy of LocalPass."
                    : "LocalPass is in Windows' list of passkey providers."
                  : "Not set up yet."
              }
            />
            <StatusRow
              title="Turned on in Windows"
              on={status.enabledInWindows}
              text={status.enabledInWindows ? "Windows offers LocalPass when you save or use a passkey." : "Windows keeps it off until you turn it on yourself."}
            />
            <StatusRow
              title="Connected to LocalPass"
              on={status.paired}
              text={status.paired ? "Windows and this app know each other." : "Windows has to be connected to this app once, with a code you confirm here."}
            />

            <div className="space-y-2 px-4 py-3">
              {step === "setup" && !status.developerMode && (
                <p className="text-xs text-amber-700 dark:text-amber-400">
                  Developer Mode is off. Turn it on in Windows Settings, System, For developers, then come back. Windows only lets an app that is not from the Store add a passkey provider that way.
                </p>
              )}
              {step === "enable" && (
                <p className="text-xs text-slate-500 dark:text-slate-400">
                  Windows opens its passkey settings. Turn LocalPass on there and confirm with Windows Hello. LocalPass cannot do this for you.
                </p>
              )}
              {step === "pair" && (
                <p className="text-xs text-slate-500 dark:text-slate-400">LocalPass shows a 6-digit code. Check that it is the same one and approve it.</p>
              )}
              {status.problem && <p className="text-xs text-amber-700 dark:text-amber-400">{status.problem}</p>}
              {canRepair && (
                <p className="text-xs text-amber-700 dark:text-amber-400">
                  {otherCopy
                    ? "To use this copy of LocalPass instead, set up again. Windows then turns LocalPass off and you turn it on again."
                    : "Windows cannot start LocalPass by itself yet (it has the location of another copy). Set up again to fix that."}
                </p>
              )}

              <div className="flex flex-wrap items-center gap-2">
                {step === "setup" && (
                  <Button variant="primary" busy={busy === "setup"} disabled={!status.developerMode || busy !== null} onClick={() => run("setup", api.windowsPasskeysSetup, "Added to Windows. Next, turn it on in Windows settings.")}>
                    Set up
                  </Button>
                )}
                {step === "enable" && (
                  <Button variant="primary" onClick={openWindowsSettings}>
                    <ExternalLink className="size-4" /> Turn on in Windows settings
                  </Button>
                )}
                {step === "pair" && (
                  <Button variant="primary" busy={busy === "pair"} disabled={busy !== null} onClick={() => run("pair", api.windowsPasskeysPair, "Windows passkeys are ready")}>
                    <Link2 className="size-4" /> Pair
                  </Button>
                )}
                {step === "done" && (
                  <Button variant="secondary" onClick={openWindowsSettings}>
                    <ExternalLink className="size-4" /> Windows passkey settings
                  </Button>
                )}
                {canRepair && (
                  <Button busy={busy === "setup"} disabled={!status.developerMode || busy !== null} onClick={() => run("setup", api.windowsPasskeysSetup, "Set up again")}>
                    Set up again
                  </Button>
                )}
                {status.packageRegistered && (
                  <Button variant={step === "done" ? "secondary" : "ghost"} busy={busy === "remove"} disabled={busy !== null} onClick={() => setRemoveOpen(true)}>
                    <Trash2 className="size-4" /> Remove
                  </Button>
                )}
              </div>
              {busy === "pair" && <p className="text-xs text-slate-500 dark:text-slate-400">Waiting for you to approve the code in the LocalPass dialog…</p>}
            </div>

            <div className="px-4 py-3 text-xs text-slate-500 dark:text-slate-400">
              Passkeys saved on your phone appear in Windows a few seconds after they sync, while LocalPass is unlocked. If you have several passkeys for one website, Windows sign-in can fail; use the browser extension there.
            </div>
          </>
        )}
      </Card>

      <Modal
        open={removeOpen}
        onClose={() => setRemoveOpen(false)}
        title="Remove Windows passkeys?"
        footer={
          <>
            <Button onClick={() => setRemoveOpen(false)}>Cancel</Button>
            <Button
              variant="danger"
              onClick={() => {
                setRemoveOpen(false);
                run("remove", () => api.windowsPasskeysRemove(forget), "Removed from Windows");
              }}
            >
              Remove
            </Button>
          </>
        }
      >
        <p className="text-sm text-slate-600 dark:text-slate-300">
          LocalPass leaves Windows' passkey list. Your passkeys stay in your vault, and the browser extension keeps working. Turn it on again later by setting it up again.
        </p>
        <label className="mt-3 flex items-start gap-2 text-sm">
          <input type="checkbox" className="mt-1" checked={forget} onChange={(e) => setForget(e.target.checked)} />
          <span>Also forget the connection between Windows and LocalPass (you would pair again).</span>
        </label>
      </Modal>
    </section>
  );
}

function StatusRow({ title, on, text }: { title: string; on: boolean; text: string }) {
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-3">
      <div className="min-w-0">
        <div className="flex items-center gap-2 text-sm font-medium">
          {title}
          <Badge tone={on ? "green" : "amber"}>{on ? "Yes" : "Not yet"}</Badge>
        </div>
        <div className="text-xs text-slate-500 dark:text-slate-400">{text}</div>
      </div>
    </div>
  );
}
