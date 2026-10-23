import { useCallback, useEffect, useRef, useState } from "react";
import { Globe, KeyRound, ShieldQuestion } from "lucide-react";
import { api, onEvent, type AppInfo, type ClientKind, type PairingRequest } from "./lib/api";
import { platformLabel } from "./lib/format";
import { Button, Modal, Spinner } from "./components/ui";
import { useToast } from "./components/Toaster";
import { Onboarding } from "./screens/Onboarding";
import { Unlock } from "./screens/Unlock";
import { Shell } from "./screens/Shell";
import { ScanOverlay } from "./components/ScanOverlay";

function applyTheme(theme: string) {
  const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

export default function App() {
  const toast = useToast();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [pairing, setPairing] = useState<PairingRequest | null>(null);
  const [browserPairing, setBrowserPairing] = useState<{
    requestId: number;
    name: string;
    code: string;
    kind: ClientKind;
  } | null>(null);
  // A browser (or the Windows provider) wants to replace a saved password or passkey.
  const [overwrite, setOverwrite] = useState<{
    requestId: number;
    kind: "login" | "passkey";
    title: string;
    username: string;
    site: string;
    savedFor: string | null;
    sameHost: boolean;
  } | null>(null);
  const [lastSyncMs, setLastSyncMs] = useState<number | null>(null);
  const lastTouch = useRef(0);

  const refresh = useCallback(() => {
    api.appInfo().then(setInfo).catch((e) => toast(String(e), "error"));
  }, [toast]);

  useEffect(refresh, [refresh]);

  // Theme follows the setting (and the OS when set to "system").
  useEffect(() => {
    const theme = info?.settings.theme ?? "system";
    applyTheme(theme);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyTheme(theme);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [info?.settings.theme]);

  // Engine events → app state and DOM events for screens.
  useEffect(() => {
    const unlisten = onEvent((e) => {
      switch (e.type) {
        case "status":
          refresh();
          if (e.status !== "unlocked") setPairing(null);
          break;
        case "vaultChanged":
          window.dispatchEvent(new Event("lp-vault-changed"));
          break;
        case "syncCompleted":
          setLastSyncMs(Date.now());
          window.dispatchEvent(new Event("lp-sync"));
          if (Math.abs(e.clockSkewMs) > 60000) toast("Another device's clock is off by over a minute; check its time settings.", "info");
          break;
        case "syncFailed":
        case "peerDiscovered":
        case "networkChanged":
          window.dispatchEvent(new Event("lp-sync"));
          break;
        case "pairingRequest":
          setPairing({ requestId: e.requestId, deviceName: e.deviceName, platform: e.platform, fingerprint: e.fingerprint });
          break;
        case "extensionPairing":
          setBrowserPairing({ requestId: e.requestId, name: e.name, code: e.code, kind: e.kind });
          break;
        case "confirmOverwrite":
          setOverwrite({
            requestId: e.requestId,
            kind: e.kind,
            title: e.title,
            username: e.username,
            site: e.site,
            savedFor: e.savedFor,
            sameHost: e.sameHost,
          });
          break;
        case "confirmOverwriteCancelled":
          // The program that asked gave up; there is nothing left to answer.
          setOverwrite((current) => (current?.requestId === e.requestId ? null : current));
          break;
        case "windowShown":
          window.dispatchEvent(new Event("lp-window-shown"));
          break;
        case "extensionPairingCancelled":
          // The program that asked gave up; there is nothing left to answer.
          setBrowserPairing((current) => (current?.requestId === e.requestId ? null : current));
          break;
        case "pairingFinished":
          setPairing(null);
          window.dispatchEvent(new Event("lp-paired"));
          if (e.device) toast("New device added");
          else if (e.error && e.error !== "declined") toast(e.error, "error");
          break;
        case "inviteWrongCode":
          toast(`Someone entered a wrong code (${e.attemptsLeft} attempt${e.attemptsLeft === 1 ? "" : "s"} left)`, "error");
          break;
        case "forkDetected":
          toast("A device sent conflicting history. Check Devices for a duplicated or restored device.", "error");
          break;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [refresh, toast]);

  // A phone sleeps in the background: on return, pick up a lock that happened meanwhile
  // and sync right away.
  const isAndroid = info?.platform === "android";
  useEffect(() => {
    if (!isAndroid) return;
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      refresh();
      api.syncNow().catch(() => {});
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [isAndroid, refresh]);

  // Activity resets the auto-lock timer (throttled).
  useEffect(() => {
    const onActivity = () => {
      const now = Date.now();
      if (now - lastTouch.current > 20000) {
        lastTouch.current = now;
        api.touch().catch(() => {});
      }
    };
    window.addEventListener("pointerdown", onActivity);
    window.addEventListener("keydown", onActivity);
    return () => {
      window.removeEventListener("pointerdown", onActivity);
      window.removeEventListener("keydown", onActivity);
    };
  }, []);

  const respondBrowser = (approve: boolean) => {
    if (!browserPairing) return;
    api.brokerRespond(browserPairing.requestId, approve).catch(() => {});
    setBrowserPairing(null);
    if (approve) {
      toast(browserPairing.kind === "provider" ? "Windows passkey provider connected" : "Browser connected");
      window.dispatchEvent(new Event("lp-browser-paired"));
    }
  };

  const respondOverwrite = (replace: boolean) => {
    if (!overwrite) return;
    api.brokerRespond(overwrite.requestId, replace).catch(() => {});
    setOverwrite(null);
  };

  const respond = (approve: boolean) => {
    if (!pairing) return;
    api.inviteRespond(pairing.requestId, approve).catch(() => {});
    setPairing(null);
  };

  if (!info) {
    return (
      <div className="flex h-full items-center justify-center">
        <Spinner />
      </div>
    );
  }

  return (
    <>
      {info.status === "noVault" && <Onboarding info={info} onDone={refresh} />}
      {info.status === "locked" && <Unlock info={info} onUnlocked={refresh} />}
      {info.status === "unlocked" && <Shell info={info} lastSyncMs={lastSyncMs} onInfoChange={refresh} />}
      <ScanOverlay />

      <Modal
        open={!!browserPairing}
        onClose={() => respondBrowser(false)}
        title={browserPairing?.kind === "provider" ? "Connect the Windows passkey provider?" : "Connect a browser?"}
        footer={
          <>
            <Button variant="ghost" onClick={() => respondBrowser(false)}>
              Deny
            </Button>
            <Button variant="primary" onClick={() => respondBrowser(true)}>
              Connect
            </Button>
          </>
        }
      >
        {browserPairing && (
          <div className="space-y-3 text-center">
            <Globe className="mx-auto size-10 text-indigo-600" />
            {browserPairing.kind === "provider" ? (
              <p>
                <b>{browserPairing.name}</b> (the Windows passkey provider) wants to use the passkeys in your vault when you sign in
                to apps and websites in Windows. It cannot read your passwords.
              </p>
            ) : (
              <p>
                <b>{browserPairing.name}</b> wants to fill passwords and passkeys from your vault.
              </p>
            )}
            <p className="text-sm text-slate-500">
              Connect only if {browserPairing.kind === "provider" ? "the Windows passkey provider" : "the extension"} shows this same code:
              <br />
              <span className="font-mono text-2xl tracking-widest text-slate-800 dark:text-slate-200">{browserPairing.code}</span>
            </p>
          </div>
        )}
      </Modal>

      <Modal
        open={!!overwrite}
        onClose={() => respondOverwrite(false)}
        title={overwrite?.kind === "passkey" ? "Replace this passkey?" : "Replace the saved password?"}
        footer={
          <>
            <Button variant="ghost" onClick={() => respondOverwrite(false)}>
              Keep what I have
            </Button>
            <Button variant="primary" onClick={() => respondOverwrite(true)}>
              Replace
            </Button>
          </>
        }
      >
        {overwrite && (
          <div className="space-y-3 text-center">
            <KeyRound className="mx-auto size-10 text-indigo-600" />
            {overwrite.kind === "passkey" ? (
              <p>
                <b>{overwrite.site}</b> asked to create a passkey for <b>{overwrite.username || "this account"}</b>, and you already
                have one for that account. A new passkey replaces the old one, which then stops working.
              </p>
            ) : overwrite.sameHost ? (
              <p>
                <b>{overwrite.site}</b> wants to save a different password for <b>{overwrite.title}</b>
                {overwrite.username ? ` (${overwrite.username})` : ""}. The old password stays in the item's history.
              </p>
            ) : (
              <p>
                A password was typed on <b>{overwrite.site}</b>. The login <b>{overwrite.title}</b>
                {overwrite.username ? ` (${overwrite.username})` : ""} is saved for <b>{overwrite.savedFor ?? "another address"}</b>.
                Replace its saved password with the new one?
              </p>
            )}
            <p className="text-sm text-slate-500">Replace only if you just did this yourself.</p>
          </div>
        )}
      </Modal>

      <Modal
        open={!!pairing}
        onClose={() => respond(false)}
        title="Approve new device?"
        footer={
          <>
            <Button variant="ghost" onClick={() => respond(false)}>
              Deny
            </Button>
            <Button variant="primary" onClick={() => respond(true)}>
              Approve
            </Button>
          </>
        }
      >
        {pairing && (
          <div className="space-y-3 text-center">
            <ShieldQuestion className="mx-auto size-10 text-indigo-600" />
            <p>
              <b>{pairing.deviceName}</b> ({platformLabel(pairing.platform)}) entered your code and wants a copy of your vault.
            </p>
            <p className="text-sm text-slate-500">
              Check that this fingerprint matches the one shown on that device:
              <br />
              <span className="font-mono text-base text-slate-800 dark:text-slate-200">{pairing.fingerprint}</span>
            </p>
          </div>
        )}
      </Modal>
    </>
  );
}
