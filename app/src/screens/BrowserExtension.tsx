import { useCallback, useEffect, useState } from "react";
import { FolderOpen, Globe, Trash2 } from "lucide-react";
import { api, errorText, type BrowserStatus, type Settings as AppSettings } from "../lib/api";
import { timeAgo } from "../lib/format";
import { Button, Card, IconButton, Toggle } from "../components/ui";
import { useToast } from "../components/Toaster";

/** Settings section for the browser extension (Windows). */
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

  const connected = status.chrome || status.edge;
  const where = [status.chrome && "Chrome", status.edge && "Edge"].filter(Boolean).join(" and ");

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
            <div className="text-sm font-medium">1. Let Chrome and Edge reach LocalPass</div>
            <div className="text-xs text-slate-500 dark:text-slate-400">{connected ? `Set up for ${where}.` : "Not set up yet. This only adds a small registration for your Windows user."}</div>
          </div>
          {connected ? (
            <Button busy={busy} onClick={() => run(api.browserRemove)}>
              Remove
            </Button>
          ) : (
            <Button variant="primary" busy={busy} onClick={() => run(api.browserInstall)}>
              <Globe className="size-4" /> Set up
            </Button>
          )}
        </div>
        <div className="space-y-2 px-4 py-3">
          <div className="text-sm font-medium">2. Add the extension to your browser</div>
          <p className="text-xs text-slate-500 dark:text-slate-400">
            Open <code>chrome://extensions</code> (or <code>edge://extensions</code>), turn on Developer mode, choose Load unpacked, and pick this folder:
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
                  <IconButton label="Disconnect" onClick={() => removeClient(c.id)}>
                    <Trash2 className="size-4" />
                  </IconButton>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="px-4">
          <Toggle
            label="Ask Windows Hello for every passkey sign-in"
            description="Even when a website says it doesn't need it."
            checked={settings.passkeyVerifyEveryTime}
            onChange={(v) => onChange({ passkeyVerifyEveryTime: v })}
          />
        </div>
      </Card>
    </section>
  );
}
