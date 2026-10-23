import { useEffect, useState, type ReactNode } from "react";
import { AlertTriangle, ArrowLeft, Copy, ExternalLink, Eye, EyeOff, Fingerprint, History, Pencil, RotateCcw, Star, Trash2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, errorText, type ItemView } from "../lib/api";
import { formatDate, openableUrl, timeAgo } from "../lib/format";
import { Badge, Button, IconButton, Spinner } from "../components/ui";
import { TotpCode } from "../components/TotpCode";
import { useToast } from "../components/Toaster";

const fieldLabels: Record<string, string> = {
  password: "Password",
  username: "Username",
  title: "Title",
  notes: "Notes",
  totp: "2FA secret",
  urls: "Websites",
};

function Row({ label, children, actions }: { label: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="group flex items-center gap-3 border-b border-slate-100 py-3 last:border-0 dark:border-slate-800">
      <div className="min-w-0 flex-1">
        <div className="text-xs font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">{label}</div>
        <div className="mt-0.5 min-w-0 break-words text-sm">{children}</div>
      </div>
      {actions && <div className="flex shrink-0 items-center">{actions}</div>}
    </div>
  );
}

export function ItemDetail({ id, onEdit, onBack, onGone }: { id: string; onEdit: () => void; onBack: () => void; onGone: () => void }) {
  const toast = useToast();
  const [item, setItem] = useState<ItemView | null>(null);
  const [shown, setShown] = useState<Record<string, boolean>>({});
  const [showHistory, setShowHistory] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    api
      .getItem(id)
      .then((v) => (setItem(v), setError(null)))
      .catch((e) => setError(errorText(e)));

  useEffect(() => {
    setShown({});
    setShowHistory(false);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  // Reload when sync brings changes.
  useEffect(() => {
    const onChange = () => load();
    window.addEventListener("lp-vault-changed", onChange);
    return () => window.removeEventListener("lp-vault-changed", onChange);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  if (error) return <p className="p-6 text-sm text-rose-600">{error}</p>;
  if (!item) return <Spinner className="m-6" />;

  const copy = (field: string, what: string) =>
    api
      .copyField(item.id, field)
      .then((s) => toast(s ? `${what} copied · clears in ${s}s` : `${what} copied`))
      .catch((e) => toast(errorText(e), "error"));
  const copyButton = (field: string, what: string) => (
    <IconButton label={`Copy ${what.toLowerCase()}`} onClick={() => copy(field, what)}>
      <Copy className="size-4" />
    </IconButton>
  );
  const toggle = (k: string) => setShown((s) => ({ ...s, [k]: !s[k] }));
  const secret = (k: string, value: string) => (shown[k] ? <span className="secret">{value}</span> : <span className="secret text-slate-400">••••••••••••</span>);

  const trash = async () => {
    await api.trashItem(item.id);
    toast("Moved to trash");
    onGone();
  };
  const purge = async () => {
    if (await ask(`Delete “${item.title}” permanently from all your devices?`, { title: "Delete forever", kind: "warning" })) {
      await api.purgeItem(item.id);
      toast("Deleted");
      onGone();
    }
  };

  return (
    <div className="mx-auto max-w-2xl p-4 sm:p-6">
      <div className="mb-4 flex items-start gap-2">
        <IconButton label="Back" onClick={onBack} className="md:hidden">
          <ArrowLeft className="size-5" />
        </IconButton>
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-xl font-semibold">{item.title}</h2>
          <p className="text-xs text-slate-500 dark:text-slate-400">
            Changed {timeAgo(item.updatedMs)} on {item.updatedBy}
          </p>
        </div>
        {!item.trashed && (
          <>
            <IconButton
              label={item.favorite ? "Remove from favorites" : "Add to favorites"}
              onClick={() => api.setFavorite(item.id, !item.favorite).then(load)}
            >
              <Star className={`size-5 ${item.favorite ? "fill-amber-400 text-amber-400" : ""}`} />
            </IconButton>
            <Button onClick={onEdit}>
              <Pencil className="size-4" /> Edit
            </Button>
          </>
        )}
      </div>

      {item.conflicts.length > 0 && (
        <div className="mb-4 rounded-xl bg-amber-50 p-4 ring-1 ring-amber-200 dark:bg-amber-950/40 dark:ring-amber-900">
          <div className="mb-2 flex items-center gap-2 font-medium text-amber-900 dark:text-amber-200">
            <AlertTriangle className="size-4" /> Changed on two devices at the same time
          </div>
          {item.conflicts.map((c) => (
            <div key={c.field} className="space-y-2 text-sm">
              <p className="text-amber-900/80 dark:text-amber-200/80">
                {fieldLabels[c.field] ?? c.field}: keeping the version from <b>{c.current.device}</b>. Other version:
              </p>
              {c.alternatives.map((a) => (
                <div key={a.op} className="flex items-center gap-2 rounded-lg bg-white/70 px-3 py-2 dark:bg-slate-900/60">
                  <span className="secret min-w-0 flex-1 truncate">{shown[`c-${a.op}`] ? a.value : "••••••••"}</span>
                  <span className="shrink-0 text-xs text-slate-500">{a.device}</span>
                  <IconButton label="Show" onClick={() => toggle(`c-${a.op}`)}>
                    {shown[`c-${a.op}`] ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                  </IconButton>
                  <Button
                    onClick={() =>
                      api
                        .resolveConflict(item.id, c.field, a.op)
                        .then(() => (toast("Kept the other version"), load()))
                        .catch((e) => toast(errorText(e), "error"))
                    }
                  >
                    Use this
                  </Button>
                </div>
              ))}
            </div>
          ))}
        </div>
      )}

      <div className="rounded-xl bg-white px-4 shadow-sm ring-1 ring-slate-200 dark:bg-slate-900 dark:ring-slate-800">
        {item.username && (
          <Row label="Username" actions={copyButton("username", "Username")}>
            {item.username}
          </Row>
        )}
        {item.password && (
          <Row
            label="Password"
            actions={
              <>
                <IconButton label={shown.password ? "Hide password" : "Show password"} onClick={() => toggle("password")}>
                  {shown.password ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                </IconButton>
                {copyButton("password", "Password")}
              </>
            }
          >
            {secret("password", item.password)}
          </Row>
        )}
        {item.totp && (
          <Row label="2FA code">
            <TotpCode itemId={item.id} />
          </Row>
        )}
        {item.urls.map((u, i) => {
          const href = openableUrl(u);
          return (
            <Row
              key={i}
              label={i === 0 ? "Website" : "Website " + (i + 1)}
              actions={
                href && (
                  <IconButton label="Open in browser" onClick={() => openUrl(href)}>
                    <ExternalLink className="size-4" />
                  </IconButton>
                )
              }
            >
              <span className="text-indigo-700 dark:text-indigo-300">{u}</span>
            </Row>
          );
        })}
        {item.passkey && (
          <Row label="Passkey">
            <div className="flex items-center gap-2">
              <Fingerprint className="size-4 shrink-0 text-indigo-600" />
              <span>
                {item.passkey.userName || item.passkey.userDisplayName || "Passkey"} on {item.passkey.rpId}
              </span>
            </div>
            <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
              {item.passkey.createdMs ? `Created ${formatDate(item.passkey.createdMs)}. ` : ""}The private key stays inside LocalPass and syncs encrypted between your devices.
            </p>
          </Row>
        )}
        {item.card && (
          <>
            {item.card.holder && <Row label="Cardholder">{item.card.holder}</Row>}
            {item.card.number && (
              <Row
                label={item.card.brand || "Card number"}
                actions={
                  <>
                    <IconButton label="Show" onClick={() => toggle("card")}>
                      {shown.card ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                    </IconButton>
                    {copyButton("card.number", "Card number")}
                  </>
                }
              >
                {secret("card", item.card.number.replace(/(\d{4})(?=\d)/g, "$1 "))}
              </Row>
            )}
            {(item.card.expMonth || item.card.expYear) && (
              <Row label="Expires">
                {item.card.expMonth}/{item.card.expYear}
              </Row>
            )}
            {item.card.cvv && (
              <Row label="Security code" actions={copyButton("card.cvv", "Security code")}>
                {secret("cvv", item.card.cvv)}
              </Row>
            )}
          </>
        )}
        {item.custom.map((f) => (
          <Row
            key={f.id}
            label={f.label || "Field"}
            actions={
              <>
                {f.hidden && (
                  <IconButton label="Show" onClick={() => toggle(`f-${f.id}`)}>
                    {shown[`f-${f.id}`] ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                  </IconButton>
                )}
                {copyButton(`custom.${f.id}`, f.label || "Value")}
              </>
            }
          >
            {f.hidden ? secret(`f-${f.id}`, f.value) : f.value}
          </Row>
        ))}
        {item.notes && (
          <Row label="Notes">
            <p className="whitespace-pre-wrap">{item.notes}</p>
          </Row>
        )}
        {!item.username && !item.password && !item.notes && !item.urls.length && !item.card && !item.passkey && !item.custom.length && (
          <p className="py-6 text-center text-sm text-slate-500">No details yet.</p>
        )}
      </div>

      {item.passwordHistory.length > 0 && (
        <div className="mt-4">
          <button className="flex items-center gap-2 text-sm text-slate-600 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-200" onClick={() => setShowHistory((s) => !s)}>
            <History className="size-4" /> Password history ({item.passwordHistory.length})
          </button>
          {showHistory && (
            <ul className="mt-2 space-y-1 rounded-xl bg-white p-2 text-sm ring-1 ring-slate-200 dark:bg-slate-900 dark:ring-slate-800">
              {item.passwordHistory.map((h, i) => (
                <li key={i} className="flex items-center gap-2 rounded-lg px-2 py-1.5">
                  <span className="secret min-w-0 flex-1 truncate">{shown[`h-${i}`] ? h.value : "••••••••"}</span>
                  <span className="shrink-0 text-xs text-slate-500">
                    {formatDate(h.setMs)} · {h.device}
                  </span>
                  <IconButton label="Show" onClick={() => toggle(`h-${i}`)}>
                    {shown[`h-${i}`] ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                  </IconButton>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="mt-6 flex flex-wrap items-center gap-2">
        {item.trashed ? (
          <>
            <Badge tone="rose">In trash</Badge>
            <Button onClick={() => api.restoreItem(item.id).then(() => (toast("Restored"), load()))}>
              <RotateCcw className="size-4" /> Restore
            </Button>
            <Button variant="danger" onClick={purge}>
              <Trash2 className="size-4" /> Delete forever
            </Button>
          </>
        ) : (
          <Button variant="ghost" onClick={trash} className="text-rose-600 dark:text-rose-400">
            <Trash2 className="size-4" /> Move to trash
          </Button>
        )}
        <span className="ml-auto text-xs text-slate-400">Created {formatDate(item.createdMs)}</span>
      </div>
    </div>
  );
}
