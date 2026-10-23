import { useCallback, useEffect, useState } from "react";
import { api, errorText, type AndroidSetup as Setup, type SetupState, type SetupTarget } from "../lib/api";
import { Badge, Button, Card } from "../components/ui";
import { useToast } from "../components/Toaster";

/**
 * Settings section for Android: turn LocalPass on as the autofill service and the passkey provider,
 * which only the system's own settings can do. The status is read again whenever the user comes
 * back from those pages. Android does not let an app see whether it is a passkey provider, so that
 * row never shows a status. A warning row appears when the phone has no screen lock, because
 * LocalPass refuses passkeys then (it could not check that the owner is present).
 */
export function AndroidSetup() {
  const toast = useToast();
  const [setup, setSetup] = useState<Setup | null>(null);

  const reload = useCallback(() => {
    api.androidSetupStatus().then(setSetup).catch(() => {});
  }, []);

  useEffect(() => {
    reload();
    const onVisible = () => document.visibilityState === "visible" && reload();
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", reload);
    return () => {
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", reload);
    };
  }, [reload]);

  const open = (target: SetupTarget) => api.androidOpenSetup(target).catch((e) => toast(errorText(e), "error"));

  if (!setup) return null;

  return (
    <section className="space-y-2">
      <h2 className="px-1 text-sm font-semibold text-slate-500 uppercase dark:text-slate-400">Autofill and passkeys</h2>
      <Card className="divide-y divide-slate-100 py-1 dark:divide-slate-800">
        <Row
          title="Autofill passwords"
          state={setup.autofill}
          text={
            setup.autofill === "unsupported"
              ? "This phone does not offer autofill services."
              : setup.autofill === "on"
                ? "LocalPass fills and saves logins in your browser."
                : "Not on yet. Choose LocalPass as the preferred service."
          }
          action={setup.autofill === "unsupported" ? null : { label: setup.autofill === "on" ? "Change" : "Turn on", primary: setup.autofill !== "on", onClick: () => open("autofill") }}
        />
        <Row
          title="Passkeys"
          state={setup.credentialProvider}
          text={setup.credentialProvider === "unsupported" ? "Passkeys need Android 14 or later." : "Open settings and switch LocalPass on."}
          action={setup.credentialProvider === "unsupported" ? null : { label: "Open settings", primary: true, onClick: () => open("credentials") }}
        />
        {setup.credentialProvider !== "unsupported" && setup.screenLock === "off" && (
          <Row
            title="Screen lock"
            state="off"
            text="This phone has no screen lock. LocalPass will not create or use passkeys until you set a PIN, pattern, password or fingerprint. Passwords are not affected."
            action={{ label: "Open settings", primary: true, onClick: () => open("security") }}
          />
        )}
        {setup.localNetwork !== "unsupported" && (
          <Row
            title="Local network"
            state={setup.localNetwork}
            text={setup.localNetwork === "on" ? "Allowed, so your devices can find each other." : "Not allowed, so syncing and pairing with your other devices will not work. Tap Allow, then Permissions, then Local network."}
            action={setup.localNetwork === "on" ? null : { label: "Allow", primary: true, onClick: () => open("app") }}
          />
        )}
        <div className="space-y-1.5 px-4 py-3 text-xs text-slate-500 dark:text-slate-400">
          <p>
            <b>Passkeys:</b> on Android 14 and later, choosing LocalPass as the preferred service usually turns passkeys on too. If not, switch LocalPass on under Additional services.
          </p>
          <p>
            <b>Chrome:</b> open Chrome Settings, Autofill services, choose "Autofill using another service", then restart Chrome.
          </p>
          <p>
            <b>Saving:</b> after you sign in to a website, Android asks whether to save the password. Tap Save.
          </p>
          <p>
            <b>Limits:</b> works in web pages in a browser such as Chrome or Edge. Other apps are not supported.
          </p>
        </div>
      </Card>
    </section>
  );
}

function Row({
  title,
  state,
  text,
  action,
}: {
  title: string;
  state: SetupState;
  text: string;
  action: { label: string; primary: boolean; onClick: () => void } | null;
}) {
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-3">
      <div className="min-w-0">
        <div className="flex items-center gap-2 text-sm font-medium">
          {title}
          {state === "on" && <Badge tone="green">On</Badge>}
          {state === "off" && <Badge tone="amber">Off</Badge>}
        </div>
        <div className="text-xs text-slate-500 dark:text-slate-400">{text}</div>
      </div>
      {action && (
        <Button variant={action.primary ? "primary" : "secondary"} onClick={action.onClick} className="shrink-0">
          {action.label}
        </Button>
      )}
    </div>
  );
}
