import { useCallback, useEffect, useState } from "react";
import { Ban, Laptop, Monitor, Plus, RefreshCw, Smartphone, Wifi, WifiOff } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, errorText, type DeviceInfo, type InviteInfo, type SyncStatus } from "../lib/api";
import { platformLabel, timeAgo } from "../lib/format";
import { Badge, Button, Card, ErrorText, Modal, TextInput } from "../components/ui";
import { useToast } from "../components/Toaster";

function DeviceIcon({ platform }: { platform: string }) {
  if (platform === "android" || platform === "ios") return <Smartphone className="size-5" />;
  if (platform === "windows" || platform === "macos" || platform === "linux") return <Laptop className="size-5" />;
  return <Monitor className="size-5" />;
}

export function Devices() {
  const toast = useToast();
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [invite, setInvite] = useState<InviteInfo | null>(null);
  const [inviteError, setInviteError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [peerAddr, setPeerAddr] = useState("");
  const [peerBusy, setPeerBusy] = useState(false);

  const reload = useCallback(() => {
    api.devices().then(setDevices).catch(() => {});
    api.syncStatus().then(setStatus).catch(() => {});
  }, []);

  useEffect(() => {
    reload();
    const t = setInterval(reload, 5000);
    const onChange = () => reload();
    window.addEventListener("lp-vault-changed", onChange);
    window.addEventListener("lp-sync", onChange);
    const onPaired = () => setInvite(null);
    window.addEventListener("lp-paired", onPaired);
    return () => {
      clearInterval(t);
      window.removeEventListener("lp-vault-changed", onChange);
      window.removeEventListener("lp-sync", onChange);
      window.removeEventListener("lp-paired", onPaired);
    };
  }, [reload]);

  const startInvite = async () => {
    setInviteError(null);
    try {
      setInvite(await api.inviteStart());
    } catch (e) {
      setInviteError(errorText(e));
    }
  };

  const closeInvite = () => {
    api.inviteCancel();
    setInvite(null);
  };

  const syncNow = async () => {
    setSyncing(true);
    try {
      const n = await api.syncNow();
      toast(n ? `Synced with ${n} device${n === 1 ? "" : "s"}` : "No other devices reachable right now", n ? "success" : "info");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSyncing(false);
      reload();
    }
  };

  const revoke = async (d: DeviceInfo) => {
    const ok = await ask(
      `Remove “${d.name}” from your vault? It will no longer be able to sync with your devices. Use this if the device is lost or you no longer use it.`,
      { title: "Remove device", kind: "warning" },
    );
    if (!ok) return;
    try {
      await api.revokeDevice(d.id);
      toast(`${d.name} removed`);
      reload();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const addPeer = async () => {
    setPeerBusy(true);
    try {
      await api.addPeer(peerAddr.trim());
      toast("Connected and synced");
      setPeerAddr("");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setPeerBusy(false);
      reload();
    }
  };

  const others = devices.filter((d) => !d.thisDevice);
  return (
    <div className="mx-auto max-w-3xl space-y-6 p-4 sm:p-6">
      <div className="flex flex-wrap items-center gap-2">
        <h1 className="flex-1 text-xl font-semibold">Devices</h1>
        <Button onClick={syncNow} busy={syncing}>
          <RefreshCw className="size-4" /> Sync now
        </Button>
        <Button variant="primary" onClick={startInvite}>
          <Plus className="size-4" /> Add device
        </Button>
      </div>
      <ErrorText>{inviteError}</ErrorText>

      <Card className="divide-y divide-slate-100 p-0 dark:divide-slate-800">
        {devices.map((d) => (
          <div key={d.id} className="flex items-center gap-3 px-4 py-3">
            <div className="relative flex size-10 shrink-0 items-center justify-center rounded-xl bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300">
              <DeviceIcon platform={d.platform} />
              {!d.thisDevice && d.status === "active" && (
                <span className={`absolute -right-0.5 -bottom-0.5 size-3 rounded-full ring-2 ring-white dark:ring-slate-900 ${d.online ? "bg-emerald-500" : "bg-slate-300 dark:bg-slate-600"}`} />
              )}
            </div>
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <span className="truncate font-medium">{d.name}</span>
                {d.thisDevice && <Badge tone="indigo">This device</Badge>}
                {d.status === "revoked" && <Badge tone="rose">Removed</Badge>}
                {d.status === "pending" && <Badge tone="amber">Needs approval</Badge>}
              </div>
              <div className="text-xs text-slate-500 dark:text-slate-400">
                {platformLabel(d.platform)}
                {!d.thisDevice && d.status === "active" && <> · {d.online ? "online now" : `last synced ${timeAgo(d.lastSyncMs)}`}</>}
                {!d.thisDevice && d.lastError && d.status === "active" && !d.online && <> · last attempt failed</>}
                {" · "}
                <span className="font-mono">{d.fingerprint}</span>
              </div>
            </div>
            {!d.thisDevice && d.status === "active" && (
              <Button variant="ghost" onClick={() => revoke(d)} className="text-rose-600 dark:text-rose-400">
                <Ban className="size-4" /> Remove
              </Button>
            )}
          </div>
        ))}
      </Card>

      {others.length === 0 && (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Only this device so far. Choose <b>Add device</b>, then on your phone or other computer pick “I already use LocalPass” and enter the code.
        </p>
      )}

      <Card>
        <div className="mb-3 flex items-center gap-2">
          {status?.running ? <Wifi className="size-4 text-emerald-600" /> : <WifiOff className="size-4 text-slate-400" />}
          <h2 className="font-medium">Network</h2>
        </div>
        {status?.running ? (
          <div className="space-y-3 text-sm">
            <p className="text-slate-600 dark:text-slate-400">
              This device can be reached at{" "}
              {status.listenAddrs.length ? status.listenAddrs.map((a) => <code key={a} className="mr-1 rounded bg-slate-100 px-1.5 py-0.5 text-xs dark:bg-slate-800">{a}</code>) : "no network address (not connected?)"}
            </p>
            {status.peers.filter((p) => p.visible).length > 0 && (
              <p className="text-slate-600 dark:text-slate-400">
                Seeing {status.peers.filter((p) => p.visible).map((p) => p.name ?? "a new device").join(", ")} on this network.
              </p>
            )}
            <div className="flex gap-2">
              <TextInput value={peerAddr} onChange={(e) => setPeerAddr(e.target.value)} placeholder="Other device's address, e.g. 192.168.1.20" />
              <Button onClick={addPeer} disabled={!peerAddr.trim()} busy={peerBusy}>
                Connect
              </Button>
            </div>
            <p className="text-xs text-slate-500">
              Devices find each other automatically. If a network blocks that (common on office or guest Wi-Fi), connect by address or use your phone's hotspot.
            </p>
          </div>
        ) : (
          <p className="text-sm text-slate-500">Sync isn't running.</p>
        )}
      </Card>

      <Modal open={!!invite} onClose={closeInvite} title="Add a device">
        {invite && <InviteBody invite={invite} />}
      </Modal>
    </div>
  );
}

function InviteBody({ invite }: { invite: InviteInfo }) {
  const [left, setLeft] = useState(invite.expiresInSecs);
  useEffect(() => {
    const t = setInterval(() => setLeft((l) => Math.max(0, l - 1)), 1000);
    return () => clearInterval(t);
  }, []);
  const qrSrc = `data:image/svg+xml;utf8,${encodeURIComponent(invite.qrSvg)}`;
  return (
    <div className="space-y-4 text-center">
      <p className="text-sm text-slate-600 dark:text-slate-400">
        On the new device, choose <b>I already use LocalPass</b> and enter this code, or scan the QR code with your phone.
      </p>
      <div className="font-mono text-4xl font-semibold tracking-[0.2em] tabular-nums">{invite.codeDisplay}</div>
      <img src={qrSrc} alt="Pairing QR code" className="mx-auto size-52 rounded-lg bg-white p-2" />
      {invite.addrs.length > 0 && (
        <p className="text-xs text-slate-500">
          If it can't find this device, enter its address: <span className="font-mono">{invite.addrs.join(", ")}</span>
        </p>
      )}
      <p className={`text-xs ${left < 30 ? "text-rose-600" : "text-slate-500"}`}>
        {left > 0 ? `Code expires in ${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}` : "Code expired: close and start again"}
      </p>
      <p className="text-xs text-slate-500">You'll be asked to approve the new device here before it gets your vault.</p>
    </div>
  );
}
