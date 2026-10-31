import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertTriangle, Copy, CreditCard, Fingerprint, KeyRound, ListChecks, Plus, RotateCcw, Search, ShieldCheck, Star, StickyNote, Trash2, User, X } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, errorText, type FolderView, type ItemKind, type ItemSummary } from "../lib/api";
import { hostOf } from "../lib/format";
import { Button, EmptyState, IconButton } from "../components/ui";
import { useToast } from "../components/Toaster";
import { ItemDetail } from "./ItemDetail";
import { ItemEditor } from "./ItemEditor";

export type Section = { kind: "all" } | { kind: "favorites" } | { kind: "folder"; id: string } | { kind: "trash" };

const kindIcon: Record<ItemKind, typeof KeyRound> = { login: KeyRound, note: StickyNote, card: CreditCard, identity: User, passkey: Fingerprint };

// The type chips, in the order they are shown. A chip appears when the list has items of that type.
const kindLabel: Record<ItemKind, string> = { login: "Logins", note: "Notes", card: "Cards", identity: "Identities", passkey: "Passkeys" };
const kindOrder: ItemKind[] = ["login", "note", "card", "identity", "passkey"];

type Pane = { mode: "none" } | { mode: "view"; id: string } | { mode: "edit"; id: string | null };

export function Vault({ section, folders, title }: { section: Section; folders: FolderView[]; title: string }) {
  const toast = useToast();
  const [search, setSearch] = useState("");
  const [items, setItems] = useState<ItemSummary[]>([]);
  const [pane, setPane] = useState<Pane>({ mode: "none" });
  const [kindFilter, setKindFilter] = useState<ItemKind | "all">("all");
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [working, setWorking] = useState(false);

  const query = useMemo(
    () => ({
      search,
      trash: section.kind === "trash",
      favorites: section.kind === "favorites",
      folder: section.kind === "folder" ? section.id : null,
    }),
    [search, section],
  );

  const reload = useCallback(() => {
    api.listItems(query).then(setItems).catch((e) => toast(errorText(e), "error"));
  }, [query, toast]);

  useEffect(reload, [reload]);
  useEffect(() => {
    window.addEventListener("lp-vault-changed", reload);
    return () => window.removeEventListener("lp-vault-changed", reload);
  }, [reload]);
  useEffect(() => {
    setPane({ mode: "none" });
    setKindFilter("all");
    setSelecting(false);
    setSelected(new Set());
  }, [section]);

  const kindCounts = useMemo(() => {
    const counts: Partial<Record<ItemKind, number>> = {};
    for (const i of items) counts[i.kind] = (counts[i.kind] ?? 0) + 1;
    return counts;
  }, [items]);
  const visible = useMemo(() => (kindFilter === "all" ? items : items.filter((i) => i.kind === kindFilter)), [items, kindFilter]);
  // Only what is on screen is ever acted on: an item that a filter or a search hides stays untouched.
  const chosen = useMemo(() => visible.filter((i) => selected.has(i.id)), [visible, selected]);

  const stopSelecting = useCallback(() => {
    setSelecting(false);
    setSelected(new Set());
  }, []);
  const toggleSelected = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const allChosen = visible.length > 0 && chosen.length === visible.length;

  const bulk = async (what: "trash" | "restore" | "purge") => {
    const ids = chosen.map((i) => i.id);
    if (ids.length === 0) return;
    const n = ids.length;
    const noun = `${n} item${n === 1 ? "" : "s"}`;
    if (what === "purge" && !(await ask(`Delete ${noun} permanently, on all your devices? This cannot be undone.`, { title: "Delete forever", kind: "warning" }))) return;
    const run = what === "trash" ? api.trashItem : what === "restore" ? api.restoreItem : api.purgeItem;
    setWorking(true);
    let failed = 0;
    // One at a time: each change is written to the vault's log in order.
    for (const id of ids) {
      try {
        await run(id);
      } catch {
        failed += 1;
      }
    }
    setWorking(false);
    const done = n - failed;
    const doneNoun = `${done} item${done === 1 ? "" : "s"}`;
    if (failed > 0) toast(`${failed} of ${n} could not be changed`, "error");
    else toast(what === "trash" ? `Moved ${doneNoun} to trash` : what === "restore" ? `Restored ${doneNoun}` : `Deleted ${doneNoun}`);
    if (pane.mode !== "none" && pane.id && ids.includes(pane.id)) setPane({ mode: "none" });
    stopSelecting();
    reload();
  };

  // Ctrl+F focuses search, Ctrl+N creates an item.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && selecting) {
        stopSelecting();
        return;
      }
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key === "f") {
        e.preventDefault();
        document.getElementById("vault-search")?.focus();
      } else if (e.key === "n" && section.kind !== "trash") {
        e.preventDefault();
        setPane({ mode: "edit", id: null });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [section, selecting, stopSelecting]);

  const quickCopy = (id: string, field: string, what: string) =>
    api
      .copyField(id, field)
      .then((s) => toast(s ? `${what} copied · clears in ${s}s` : `${what} copied`))
      .catch((e) => toast(errorText(e), "error"));

  const emptyTrash = async () => {
    if (await ask("Delete everything in the trash permanently, on all your devices?", { title: "Empty trash", kind: "warning" })) {
      const n = await api.emptyTrash();
      toast(`Deleted ${n} item${n === 1 ? "" : "s"}`);
      reload();
    }
  };

  const showList = pane.mode === "none";
  return (
    <div className="flex h-full min-h-0">
      <section className={`${showList ? "flex" : "hidden"} w-full flex-col border-r border-slate-200 md:flex md:w-80 lg:w-96 dark:border-slate-800`}>
        <div className="space-y-3 border-b border-slate-200 p-3 dark:border-slate-800">
          <div className="flex items-center gap-2">
            <h1 className="flex-1 truncate text-lg font-semibold">{title}</h1>
            {items.length > 0 && !selecting && (
              <Button variant="ghost" onClick={() => setSelecting(true)}>
                <ListChecks className="size-4" /> Select
              </Button>
            )}
            {section.kind === "trash" ? (
              items.length > 0 &&
              !selecting && (
                <Button variant="ghost" onClick={emptyTrash} className="text-rose-600">
                  <Trash2 className="size-4" /> Empty
                </Button>
              )
            ) : (
              !selecting && (
                <Button variant="primary" onClick={() => setPane({ mode: "edit", id: null })}>
                  <Plus className="size-4" /> New
                </Button>
              )
            )}
          </div>
          <div className="relative">
            <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-slate-400" />
            <input
              id="vault-search"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search"
              className="w-full rounded-lg border-0 bg-slate-100 py-2 pr-3 pl-9 text-sm placeholder:text-slate-400 focus:bg-white focus:ring-2 focus:ring-indigo-500 focus:outline-none dark:bg-slate-900 dark:focus:bg-slate-900"
            />
          </div>
          {(items.length > 0 || kindFilter !== "all") && (
            <div className="flex flex-wrap gap-1.5" role="tablist" aria-label="Item type">
              {(["all", ...kindOrder] as const)
                .filter((k) => k === "all" || kindCounts[k] || k === kindFilter)
                .map((k) => {
                  const on = kindFilter === k;
                  return (
                    <button
                      key={k}
                      role="tab"
                      aria-selected={on}
                      onClick={() => setKindFilter(k)}
                      className={`rounded-full px-2.5 py-1 text-xs font-medium transition-colors ${
                        on
                          ? "bg-indigo-600 text-white"
                          : "bg-slate-100 text-slate-600 hover:bg-slate-200 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-700"
                      }`}
                    >
                      {k === "all" ? "All" : kindLabel[k]} <span className="opacity-70">{k === "all" ? items.length : (kindCounts[k] ?? 0)}</span>
                    </button>
                  );
                })}
            </div>
          )}
          {selecting && (
            <div className="flex flex-wrap items-center gap-2 rounded-lg bg-indigo-50 px-3 py-2 dark:bg-indigo-950/50">
              <label className="flex cursor-pointer items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  className="size-4 rounded accent-indigo-600"
                  checked={allChosen}
                  onChange={() => setSelected(allChosen ? new Set() : new Set(visible.map((i) => i.id)))}
                />
                {chosen.length > 0 ? `${chosen.length} selected` : "Select all"}
              </label>
              <span className="flex-1" />
              {section.kind === "trash" ? (
                <>
                  <Button variant="secondary" busy={working} disabled={chosen.length === 0} onClick={() => bulk("restore")}>
                    <RotateCcw className="size-4" /> Restore
                  </Button>
                  <Button variant="danger" busy={working} disabled={chosen.length === 0} onClick={() => bulk("purge")}>
                    <Trash2 className="size-4" /> Delete
                  </Button>
                </>
              ) : (
                <Button variant="danger" busy={working} disabled={chosen.length === 0} onClick={() => bulk("trash")}>
                  <Trash2 className="size-4" /> Move to trash
                </Button>
              )}
              <Button variant="ghost" disabled={working} onClick={stopSelecting}>
                <X className="size-4" /> Done
              </Button>
            </div>
          )}
        </div>
        <ul className="min-h-0 flex-1 overflow-y-auto p-2">
          {visible.map((i) => {
            const Icon = kindIcon[i.kind] ?? KeyRound;
            const active = !selecting && pane.mode !== "none" && "id" in pane && pane.id === i.id;
            const picked = selected.has(i.id);
            const open = () => (selecting ? toggleSelected(i.id) : setPane({ mode: "view", id: i.id }));
            return (
              <li key={i.id}>
                <div
                  role="button"
                  tabIndex={0}
                  onClick={open}
                  onKeyDown={(e) => (e.key === "Enter" || (selecting && e.key === " ")) && (e.preventDefault(), open())}
                  className={`group flex cursor-pointer items-center gap-3 rounded-lg px-3 py-2.5 ${
                    active || picked ? "bg-indigo-50 dark:bg-indigo-950/50" : "hover:bg-slate-100 dark:hover:bg-slate-900"
                  }`}
                >
                  {selecting && (
                    <input
                      type="checkbox"
                      tabIndex={-1}
                      aria-label={`Select ${i.title}`}
                      className="size-4 shrink-0 rounded accent-indigo-600"
                      checked={picked}
                      onClick={(e) => e.stopPropagation()}
                      onChange={() => toggleSelected(i.id)}
                    />
                  )}
                  <div className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300">
                    <Icon className="size-4" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1.5">
                      <span className="truncate text-sm font-medium">{i.title}</span>
                      {i.favorite && <Star className="size-3 shrink-0 fill-amber-400 text-amber-400" />}
                      {i.conflicted && <AlertTriangle className="size-3 shrink-0 text-amber-500" aria-label="Conflicting edits" />}
                    </div>
                    <div className="truncate text-xs text-slate-500 dark:text-slate-400">{i.subtitle || (i.url ? hostOf(i.url) : "")}</div>
                  </div>
                  {i.hasPassword && !i.trashed && !selecting && (
                    <IconButton
                      label="Copy password"
                      className="opacity-0 group-hover:opacity-100 focus:opacity-100"
                      onClick={(e) => {
                        e.stopPropagation();
                        quickCopy(i.id, "password", "Password");
                      }}
                    >
                      <Copy className="size-4" />
                    </IconButton>
                  )}
                </div>
              </li>
            );
          })}
          {items.length > 0 && visible.length === 0 && (
            <EmptyState icon={<Search className="size-6" />} title={`No ${kindFilter === "all" ? "items" : kindLabel[kindFilter].toLowerCase()} here`} />
          )}
          {items.length === 0 && (
            <EmptyState icon={section.kind === "trash" ? <Trash2 className="size-6" /> : <ShieldCheck className="size-6" />} title={search ? "No matches" : section.kind === "trash" ? "Trash is empty" : "Nothing here yet"}>
              {!search && section.kind !== "trash" && "Add your first login, or import from another password manager in Settings."}
            </EmptyState>
          )}
        </ul>
      </section>
      <section className={`${showList ? "hidden md:block" : "block"} min-w-0 flex-1 overflow-y-auto`}>
        {pane.mode === "view" && (
          <ItemDetail
            id={pane.id}
            onEdit={() => setPane({ mode: "edit", id: pane.id })}
            onBack={() => setPane({ mode: "none" })}
            onGone={() => {
              setPane({ mode: "none" });
              reload();
            }}
          />
        )}
        {pane.mode === "edit" && (
          <ItemEditor
            id={pane.id}
            folders={folders}
            defaultFolder={section.kind === "folder" ? section.id : null}
            onCancel={() => setPane(pane.id ? { mode: "view", id: pane.id } : { mode: "none" })}
            onSaved={(id) => {
              toast("Saved");
              setPane({ mode: "view", id });
              reload();
            }}
          />
        )}
        {pane.mode === "none" && (
          <div className="hidden h-full items-center justify-center md:flex">
            <p className="text-sm text-slate-400">Select an item</p>
          </div>
        )}
      </section>
    </div>
  );
}
