import { useCallback, useEffect, useState } from "react";
import { Folder, FolderPlus, KeyRound, Laptop, Lock, Pencil, Settings as SettingsIcon, Star, Trash2, Vault as VaultIcon, X } from "lucide-react";
import { api, errorText, type AppInfo, type FolderView, type SyncStatus } from "../lib/api";
import { timeAgo } from "../lib/format";
import { Button, IconButton, Modal, TextInput } from "../components/ui";
import { useToast } from "../components/Toaster";
import { Vault, type Section } from "./Vault";
import { Devices } from "./Devices";
import { Settings } from "./Settings";

type Page = { page: "vault"; section: Section } | { page: "devices" } | { page: "settings" };

function sectionTitle(s: Section, folders: FolderView[]): string {
  switch (s.kind) {
    case "all":
      return "All items";
    case "favorites":
      return "Favorites";
    case "trash":
      return "Trash";
    case "folder":
      return folders.find((f) => f.id === s.id)?.name ?? "Folder";
  }
}

function NavButton({ active, onClick, icon: Icon, label, trailing }: { active: boolean; onClick: () => void; icon: typeof KeyRound; label: string; trailing?: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      className={`group flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm ${
        active ? "bg-indigo-50 font-medium text-indigo-700 dark:bg-indigo-950/60 dark:text-indigo-300" : "text-slate-700 hover:bg-slate-200/60 dark:text-slate-300 dark:hover:bg-slate-800"
      }`}
    >
      <Icon className="size-4 shrink-0" />
      <span className="min-w-0 flex-1 truncate text-left">{label}</span>
      {trailing}
    </button>
  );
}

export function Shell({ info, lastSyncMs, onInfoChange }: { info: AppInfo; lastSyncMs: number | null; onInfoChange: () => void }) {
  const toast = useToast();
  const [page, setPage] = useState<Page>({ page: "vault", section: { kind: "all" } });
  const [folders, setFolders] = useState<FolderView[]>([]);
  const [sync, setSync] = useState<SyncStatus | null>(null);
  const [folderEdit, setFolderEdit] = useState<{ id: string | null; name: string } | null>(null);

  const reloadFolders = useCallback(() => {
    api.folders().then(setFolders).catch(() => {});
  }, []);

  useEffect(() => {
    reloadFolders();
    window.addEventListener("lp-vault-changed", reloadFolders);
    return () => window.removeEventListener("lp-vault-changed", reloadFolders);
  }, [reloadFolders]);

  useEffect(() => {
    const load = () => api.syncStatus().then(setSync).catch(() => {});
    load();
    const t = setInterval(load, 10000);
    window.addEventListener("lp-sync", load);
    return () => {
      clearInterval(t);
      window.removeEventListener("lp-sync", load);
    };
  }, []);

  const saveFolder = async () => {
    if (!folderEdit) return;
    try {
      await api.saveFolder(folderEdit.id, folderEdit.name);
      setFolderEdit(null);
      reloadFolders();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const deleteFolder = async (id: string) => {
    await api.deleteFolder(id);
    if (page.page === "vault" && page.section.kind === "folder" && page.section.id === id) setPage({ page: "vault", section: { kind: "all" } });
    reloadFolders();
  };

  const isSection = (k: Section["kind"], id?: string) => page.page === "vault" && page.section.kind === k && (!id || (page.section.kind === "folder" && page.section.id === id));
  const online = sync?.peers.filter((p) => p.visible).length ?? 0;
  const syncLine = !sync?.running ? "Sync is off" : online > 0 ? `${online} device${online === 1 ? "" : "s"} nearby` : "No devices nearby";

  const nav = (
    <>
      <NavButton active={isSection("all")} onClick={() => setPage({ page: "vault", section: { kind: "all" } })} icon={VaultIcon} label="All items" />
      <NavButton active={isSection("favorites")} onClick={() => setPage({ page: "vault", section: { kind: "favorites" } })} icon={Star} label="Favorites" />
      <div className="flex items-center justify-between px-3 pt-4 pb-1 text-xs font-semibold text-slate-500 uppercase">
        Folders
        <button aria-label="New folder" title="New folder" onClick={() => setFolderEdit({ id: null, name: "" })} className="rounded p-1 hover:bg-slate-200/60 dark:hover:bg-slate-800">
          <FolderPlus className="size-3.5" />
        </button>
      </div>
      {folders.map((f) => (
        <NavButton
          key={f.id}
          active={isSection("folder", f.id)}
          onClick={() => setPage({ page: "vault", section: { kind: "folder", id: f.id } })}
          icon={Folder}
          label={f.name}
          trailing={
            <span className="flex items-center">
              <span className="text-xs text-slate-400 group-hover:hidden">{f.items}</span>
              <span className="hidden items-center group-hover:flex">
                <Pencil
                  className="size-3.5 text-slate-400 hover:text-slate-700"
                  onClick={(e) => {
                    e.stopPropagation();
                    setFolderEdit({ id: f.id, name: f.name });
                  }}
                />
                <X
                  className="ml-1 size-3.5 text-slate-400 hover:text-rose-600"
                  onClick={(e) => {
                    e.stopPropagation();
                    deleteFolder(f.id);
                  }}
                />
              </span>
            </span>
          }
        />
      ))}
      <div className="pt-4" />
      <NavButton active={isSection("trash")} onClick={() => setPage({ page: "vault", section: { kind: "trash" } })} icon={Trash2} label="Trash" />
      <NavButton active={page.page === "devices"} onClick={() => setPage({ page: "devices" })} icon={Laptop} label="Devices" />
      <NavButton active={page.page === "settings"} onClick={() => setPage({ page: "settings" })} icon={SettingsIcon} label="Settings" />
    </>
  );

  return (
    <div className="flex h-full flex-col md:flex-row">
      <aside className="hidden w-60 shrink-0 flex-col border-r border-slate-200 bg-slate-100/60 md:flex dark:border-slate-800 dark:bg-slate-900/40">
        <div className="flex items-center gap-2 px-4 py-4">
          <div className="flex size-8 items-center justify-center rounded-lg bg-indigo-600 text-white">
            <KeyRound className="size-4" />
          </div>
          <div className="min-w-0">
            <div className="text-sm font-semibold">LocalPass</div>
            <div className="truncate text-xs text-slate-500">{info.deviceName}</div>
          </div>
        </div>
        <nav className="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2">{nav}</nav>
        <div className="flex items-center gap-2 border-t border-slate-200 p-3 dark:border-slate-800">
          <button className="min-w-0 flex-1 text-left" onClick={() => setPage({ page: "devices" })}>
            <div className="flex items-center gap-1.5 text-xs font-medium">
              <span className={`size-2 rounded-full ${online > 0 ? "bg-emerald-500" : "bg-slate-300 dark:bg-slate-600"}`} />
              {syncLine}
            </div>
            <div className="text-xs text-slate-500">Last sync {timeAgo(lastSyncMs)}</div>
          </button>
          <IconButton label="Lock" onClick={() => api.lock()}>
            <Lock className="size-4" />
          </IconButton>
        </div>
      </aside>

      <header className="flex items-center gap-2 border-b border-slate-200 px-3 py-2 md:hidden dark:border-slate-800">
        <KeyRound className="size-5 text-indigo-600" />
        <span className="flex-1 font-semibold">LocalPass</span>
        <span className={`size-2 rounded-full ${online > 0 ? "bg-emerald-500" : "bg-slate-300"}`} title={syncLine} />
        <IconButton label="Lock" onClick={() => api.lock()}>
          <Lock className="size-4" />
        </IconButton>
      </header>

      <main className="min-h-0 min-w-0 flex-1 overflow-hidden">
        <div className="h-full overflow-y-auto">
          {page.page === "vault" && <Vault section={page.section} folders={folders} title={sectionTitle(page.section, folders)} />}
          {page.page === "devices" && <Devices />}
          {page.page === "settings" && <Settings info={info} onInfoChange={onInfoChange} />}
        </div>
      </main>

      <nav className="grid grid-cols-4 border-t border-slate-200 bg-white pb-[env(safe-area-inset-bottom)] md:hidden dark:border-slate-800 dark:bg-slate-950">
        {(
          [
            ["Items", VaultIcon, () => setPage({ page: "vault", section: { kind: "all" } }), page.page === "vault" && page.section.kind !== "trash"],
            ["Trash", Trash2, () => setPage({ page: "vault", section: { kind: "trash" } }), isSection("trash")],
            ["Devices", Laptop, () => setPage({ page: "devices" }), page.page === "devices"],
            ["Settings", SettingsIcon, () => setPage({ page: "settings" }), page.page === "settings"],
          ] as const
        ).map(([label, Icon, go, active]) => (
          <button key={label} onClick={go} className={`flex flex-col items-center gap-0.5 py-2 text-xs ${active ? "text-indigo-600 dark:text-indigo-400" : "text-slate-500"}`}>
            <Icon className="size-5" />
            {label}
          </button>
        ))}
      </nav>

      <Modal
        open={!!folderEdit}
        onClose={() => setFolderEdit(null)}
        title={folderEdit?.id ? "Rename folder" : "New folder"}
        footer={
          <>
            <Button variant="ghost" onClick={() => setFolderEdit(null)}>
              Cancel
            </Button>
            <Button variant="primary" disabled={!folderEdit?.name.trim()} onClick={saveFolder}>
              Save
            </Button>
          </>
        }
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            saveFolder();
          }}
        >
          <TextInput label="Name" value={folderEdit?.name ?? ""} onChange={(e) => setFolderEdit((f) => (f ? { ...f, name: e.target.value } : f))} autoFocus />
        </form>
      </Modal>
    </div>
  );
}
