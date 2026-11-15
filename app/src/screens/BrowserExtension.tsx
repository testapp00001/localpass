import { useCallback, useEffect, useState } from "react";
import { FolderOpen, Globe, Trash2 } from "lucide-react";
import { api, errorText, type BrowserStatus, type Settings as AppSettings } from "../lib/api";
import { timeAgo } from "../lib/format";
import { Button, Card, IconButton, Toggle } from "../components/ui";
import { useToast } from "../components/Toaster";

/** Names joined for a sentence: "A", "A and B", "A, B and C". */
function joinNames(names: string[]): string {
  return names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** Settings section for the browser extension (Chrome and Edge on Windows; Firefox and the Chromium browsers on Linux). */
export function BrowserExtension({ settings, onChange }: { settings: AppSettings; onChange: (patch: Partial<AppSettings>) => void }) {
  const toast = useToast();
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(() => {
    api.browserStatus().then(setStatus).catch(() => {});
  }, []);

  useEffect(() => {
    reload();
    window.addEventListener("lp-browser-paired", reload);
    return () => window.removeEventListener("lp-browser-paired", reload);
  }, [reload]);

  if (!status?.available) return null;

  const linux = status.os === "linux";
  const registered = status.browsers.filter((b) => b.registered).map((b) => b.name);
  const present = status.browsers.filter((b) => b.present).map((b) => b.name);
  const connected = registered.length > 0;
  // Linux: a browser first run after the set-up still needs its own file (Set up again writes it).
  const missing = linux ? status.browsers.filter((b) => b.present && !b.registered).map((b) => b.name) : [];
  const otherCopy = linux ? status.otherCopy : null;
  const found = present.length > 0 ? joinNames(present) : "your browsers";

  const run = async (action: () => Promise<BrowserStatus>) => {
    setBusy(true);
    try {
      setStatus(await action());
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const removeClient = async (id: string) => {
    try {
      await api.browserClientRemove(id);
      reload();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <section className="space-y-2">
      <h2 className="px-1 text-sm font-semibold text-slate-500 uppercase dark:text-slate-400">Browser extension</h2>
      <Card className="divide-y divide-slate-100 py-1 dark:divide-slate-800">
        <div className="flex items-center justify-between gap-4 px-4 py-3">
          <div className="min-w-0">
            <div className="text-sm font-medium">1. Let {found} reach LocalPass</div>
            <div className="text-xs text-slate-500 dark:text-slate-400">
              {otherCopy ? (
                <>
                  {connected ? `Set up for ${joinNames(registered)}. ` : ""}
                  {connected ? "Some browsers are" : "Your browsers are"} set up for another copy of LocalPass: <code className="break-all">{otherCopy}</code>
                </>
              ) : connected ? (
                `Set up for ${joinNames(registered)}.${missing.length > 0 ? ` Not yet for ${joinNames(missing)}.` : ""}`
              ) : linux ? (
                "Not set up yet. This only adds one small file to each browser's settings folder (~/.mozilla and ~/.config)."
              ) : (
                "Not set up yet. This only adds a small registration for your Windows user."
              )}
            </div>
          </div>
          <div className="flex shrink-0 gap-2">
            {otherCopy ? (
              <Button variant="primary" busy={busy} onClick={() => run(api.browserInstall)}>
                <Globe className="size-4" /> Use this copy
              </Button>
            ) : (
              missing.length > 0 &&
              connected && (
                <Button busy={busy} onClick={() => run(api.browserInstall)}>
                  <Globe className="size-4" /> Set up
                </Button>
              )
            )}
            {connected ? (
              <Button busy={busy} onClick={() => run(api.browserRemove)}>
                Remove
              </Button>
            ) : (
              !otherCopy && (
                <Button variant="primary" busy={busy} onClick={() => run(api.browserInstall)}>
                  <Globe className="size-4" /> Set up
                </Button>
              )
            )}
          </div>
        </div>
        <div className="space-y-2 px-4 py-3">
          <div className="text-sm font-medium">2. Add the extension to your browser</div>
          {linux &&
            (status.firefoxAddon ? (
              <>
                <p className="text-xs text-slate-500 dark:text-slate-400">
                  <span className="font-medium">Firefox:</span> open <code>about:addons</code>, click the gear, choose Install Add-on From File, and pick this file (signed by Mozilla, it came with LocalPass):
                </p>
                <div className="flex items-center gap-2">
                  <code className="min-w-0 flex-1 rounded bg-slate-100 px-2 py-1 text-xs break-all dark:bg-slate-800">{status.firefoxAddon}</code>
                  <Button onClick={() => api.openFirefoxAddon().catch((e) => toast(errorText(e), "error"))}>
                    <FolderOpen className="size-4" /> Open
                  </Button>
                </div>
              </>
            ) : (
              <p className="text-xs text-slate-500 dark:text-slate-400">
                <span className="font-medium">Firefox:</span> open <code>about:addons</code>, click the gear, choose Install Add-on From File, and pick the LocalPass add-on (the <code>.xpi</code> file from the release).
              </p>
            ))}
          <p className="text-xs text-slate-500 dark:text-slate-400">
            {linux ? (
              <>
                <span className="font-medium">Chrome, Chromium, Brave, Edge or Vivaldi:</span> open <code>chrome://extensions</code>, turn on Developer mode, choose Load unpacked, and pick this folder:
              </>
            ) : (
              <>
                Open <code>chrome://extensions</code> (or <code>edge://extensions</code>), turn on Developer mode, choose Load unpacked, and pick this folder:
              </>
            )}
          </p>
          {status.extensionFolder ? (
            <div className="flex items-center gap-2">
              <code className="min-w-0 flex-1 rounded bg-slate-100 px-2 py-1 text-xs break-all dark:bg-slate-800">{status.extensionFolder}</code>
              <Button onClick={() => api.openExtensionFolder().catch((e) => toast(errorText(e), "error"))}>
                <FolderOpen className="size-4" /> Open
              </Button>
            </div>
          ) : (
            <p className="text-xs text-amber-700 dark:text-amber-400">The extension has not been built yet. Run <code>bun run build</code> in the <code>extension</code> folder.</p>
          )}
          <p className="text-xs text-slate-500 dark:text-slate-400">Then click the LocalPass icon in the browser and choose Connect. You will confirm the code here.</p>
          {linux && <p className="text-xs text-slate-500 dark:text-slate-400">Browsers installed as a Flatpak or Snap cannot reach LocalPass.</p>}
        </div>
        <div className="px-4 py-3">
          <div className="mb-1 text-sm font-medium">Connected browsers</div>
          {status.clients.length === 0 ? (
            <p className="text-xs text-slate-500 dark:text-slate-400">None yet.</p>
          ) : (
            <ul className="divide-y divide-slate-100 dark:divide-slate-800">
              {status.clients.map((c) => (
                <li key={c.id} className="flex items-center justify-between gap-3 py-2">
                  <div className="min-w-0">
                    <div className="truncate text-sm">{c.name}</div>
                    <div className="text-xs text-slate-500 dark:text-slate-400">Last used {timeAgo(c.lastUsedMs)}</div>
                  </div>
                  <IconButton label={`Disconnect ${c.name}`} onClick={() => removeClient(c.id)}>
                    <Trash2 className="size-4" />
                  </IconButton>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="px-4">
          <Toggle
            label={linux ? "Ask for your master password at every passkey sign-in" : "Ask Windows Hello for every passkey sign-in"}
            description="Even when a website says it doesn't need it."
            checked={settings.passkeyVerifyEveryTime}
            onChange={(v) => onChange({ passkeyVerifyEveryTime: v })}
          />
        </div>
      </Card>
    </section>
  );
}
