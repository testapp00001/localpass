import { useEffect, useState } from "react";
import { CreditCard, KeyRound, Plus, ScanLine, StickyNote, Trash2, Wand2, X } from "lucide-react";
import { api, emptyItem, errorText, type FolderView, type ItemInput, type ItemKind } from "../lib/api";
import { Button, ErrorText, IconButton, PasswordInput, Select, TextArea, TextInput, Toggle } from "../components/ui";
import { PasswordGenerator } from "../components/PasswordGenerator";
import { canScan, scanQr } from "../lib/scan";

const kinds: { kind: ItemKind; label: string; icon: typeof KeyRound }[] = [
  { kind: "login", label: "Login", icon: KeyRound },
  { kind: "note", label: "Secure note", icon: StickyNote },
  { kind: "card", label: "Card", icon: CreditCard },
];

export function ItemEditor({
  id,
  folders,
  defaultFolder,
  onSaved,
  onCancel,
}: {
  id: string | null;
  folders: FolderView[];
  defaultFolder: string | null;
  onSaved: (id: string) => void;
  onCancel: () => void;
}) {
  const [item, setItem] = useState<ItemInput | null>(id ? null : { ...emptyItem(), folder: defaultFolder });
  const [genOpen, setGenOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!id) return;
    api
      .getItem(id)
      .then((v) =>
        setItem({
          id: v.id,
          kind: v.kind,
          title: v.title,
          username: v.username,
          password: v.password,
          urls: v.urls,
          notes: v.notes,
          totp: v.totp,
          folder: v.folder,
          favorite: v.favorite,
          card: v.card,
          custom: v.custom,
        }),
      )
      .catch((e) => setError(errorText(e)));
  }, [id]);

  if (!item) return error ? <p className="p-6 text-sm text-rose-600">{error}</p> : null;

  const set = (patch: Partial<ItemInput>) => setItem((i) => (i ? { ...i, ...patch } : i));
  const setCard = (patch: Partial<NonNullable<ItemInput["card"]>>) =>
    setItem((i) => (i ? { ...i, card: { ...(i.card ?? emptyItem("card").card!), ...patch } } : i));
  const urls = item.urls.length ? item.urls : [""];

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const saved = await api.saveItem({ ...item, urls: item.urls.filter((u) => u.trim()) });
      onSaved(saved);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <div className="mx-auto max-w-2xl p-4 sm:p-6">
      <div className="mb-4 flex items-center gap-2">
        <h2 className="flex-1 text-xl font-semibold">{id ? "Edit item" : "New item"}</h2>
        <IconButton label="Cancel" onClick={onCancel}>
          <X className="size-5" />
        </IconButton>
      </div>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        {!id && (
          <div className="grid grid-cols-3 gap-2">
            {kinds.map(({ kind, label, icon: Icon }) => (
              <button
                type="button"
                key={kind}
                onClick={() => set({ kind, card: kind === "card" ? (item.card ?? emptyItem("card").card) : null })}
                className={`flex flex-col items-center gap-1 rounded-xl px-2 py-3 text-sm ring-1 transition ${
                  item.kind === kind
                    ? "bg-indigo-50 text-indigo-700 ring-indigo-400 dark:bg-indigo-950/50 dark:text-indigo-300"
                    : "bg-white ring-slate-200 hover:ring-slate-300 dark:bg-slate-900 dark:ring-slate-800"
                }`}
              >
                <Icon className="size-5" />
                {label}
              </button>
            ))}
          </div>
        )}
        <TextInput label="Title" value={item.title} onChange={(e) => set({ title: e.target.value })} autoFocus={!id} placeholder="e.g. Gmail" />

        {item.kind === "login" && (
          <>
            <TextInput label="Username or email" value={item.username} onChange={(e) => set({ username: e.target.value })} autoComplete="off" />
            <PasswordInput
              label="Password"
              value={item.password}
              onChange={(e) => set({ password: e.target.value })}
              trailing={
                <IconButton type="button" label="Generate password" onClick={() => setGenOpen(true)} className="size-8">
                  <Wand2 className="size-4" />
                </IconButton>
              }
            />
            <div className="space-y-2">
              <label className="block text-sm font-medium text-slate-700 dark:text-slate-300">Websites</label>
              {urls.map((u, i) => (
                <div key={i} className="flex gap-2">
                  <TextInput
                    value={u}
                    placeholder="https://"
                    onChange={(e) => set({ urls: urls.map((x, j) => (j === i ? e.target.value : x)) })}
                  />
                  {urls.length > 1 && (
                    <IconButton type="button" label="Remove website" onClick={() => set({ urls: urls.filter((_, j) => j !== i) })}>
                      <Trash2 className="size-4" />
                    </IconButton>
                  )}
                </div>
              ))}
              <button type="button" className="text-sm text-indigo-600 hover:underline dark:text-indigo-400" onClick={() => set({ urls: [...urls, ""] })}>
                + Add website
              </button>
            </div>
            <div className="space-y-2">
              <TextInput
                label="2FA secret (optional)"
                value={item.totp}
                onChange={(e) => set({ totp: e.target.value })}
                placeholder="otpauth://… or base32 key"
                hint="Paste the setup key or otpauth:// link shown when you enable two-factor authentication."
                className="secret"
              />
              {canScan && (
                <Button
                  type="button"
                  variant="secondary"
                  onClick={async () => {
                    try {
                      const text = (await scanQr())?.trim();
                      if (!text) return;
                      if (text.startsWith("otpauth://")) set({ totp: text });
                      else setError("That QR code is not a two-factor setup code.");
                    } catch (e) {
                      setError(errorText(e));
                    }
                  }}
                >
                  <ScanLine className="size-4" />
                  Scan setup QR code
                </Button>
              )}
            </div>
          </>
        )}

        {item.kind === "card" && item.card && (
          <div className="grid grid-cols-2 gap-3">
            <div className="col-span-2">
              <TextInput label="Cardholder name" value={item.card.holder} onChange={(e) => setCard({ holder: e.target.value })} />
            </div>
            <div className="col-span-2">
              <TextInput label="Card number" value={item.card.number} onChange={(e) => setCard({ number: e.target.value })} inputMode="numeric" className="secret" />
            </div>
            <TextInput label="Brand" value={item.card.brand} onChange={(e) => setCard({ brand: e.target.value })} placeholder="Visa" />
            <TextInput label="Security code" value={item.card.cvv} onChange={(e) => setCard({ cvv: e.target.value })} inputMode="numeric" className="secret" />
            <TextInput label="Expiry month" value={item.card.expMonth} onChange={(e) => setCard({ expMonth: e.target.value })} placeholder="MM" />
            <TextInput label="Expiry year" value={item.card.expYear} onChange={(e) => setCard({ expYear: e.target.value })} placeholder="YYYY" />
          </div>
        )}

        <TextArea label="Notes" value={item.notes} onChange={(e) => set({ notes: e.target.value })} rows={item.kind === "note" ? 8 : 3} />

        <div className="space-y-2">
          <label className="block text-sm font-medium text-slate-700 dark:text-slate-300">Custom fields</label>
          {item.custom.map((f, i) => (
            <div key={f.id ?? `new-${i}`} className="flex flex-wrap items-center gap-2 sm:flex-nowrap">
              <TextInput
                value={f.label}
                placeholder="Label"
                onChange={(e) => set({ custom: item.custom.map((x, j) => (j === i ? { ...x, label: e.target.value } : x)) })}
                className="sm:w-40"
              />
              <TextInput
                value={f.value}
                placeholder="Value"
                type={f.hidden ? "password" : "text"}
                onChange={(e) => set({ custom: item.custom.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)) })}
              />
              <label className="flex shrink-0 items-center gap-1 text-xs text-slate-500">
                <input
                  type="checkbox"
                  checked={f.hidden}
                  onChange={(e) => set({ custom: item.custom.map((x, j) => (j === i ? { ...x, hidden: e.target.checked } : x)) })}
                />
                Hidden
              </label>
              <IconButton type="button" label="Remove field" onClick={() => set({ custom: item.custom.filter((_, j) => j !== i) })}>
                <Trash2 className="size-4" />
              </IconButton>
            </div>
          ))}
          <button
            type="button"
            className="inline-flex items-center gap-1 text-sm text-indigo-600 hover:underline dark:text-indigo-400"
            onClick={() => set({ custom: [...item.custom, { id: null, label: "", value: "", hidden: false }] })}
          >
            <Plus className="size-4" /> Add field
          </button>
        </div>

        <Select label="Folder" value={item.folder ?? ""} onChange={(e) => set({ folder: e.target.value || null })}>
          <option value="">No folder</option>
          {folders.map((f) => (
            <option key={f.id} value={f.id}>
              {f.name}
            </option>
          ))}
        </Select>
        <Toggle label="Favorite" checked={item.favorite} onChange={(v) => set({ favorite: v })} />

        <ErrorText>{error}</ErrorText>
        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" busy={busy} disabled={!item.title.trim()}>
            Save
          </Button>
        </div>
      </form>
      <PasswordGenerator open={genOpen} onClose={() => setGenOpen(false)} onUse={(p) => set({ password: p })} />
    </div>
  );
}
