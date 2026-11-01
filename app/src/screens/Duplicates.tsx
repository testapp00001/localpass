import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Star } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, errorText, type AppInfo, type DuplicateGroup } from "../lib/api";
import { timeAgo } from "../lib/format";
import { lostWithTrash } from "../lib/duplicates";
import { Badge, Button, Card, ErrorText, IconButton, Toggle } from "../components/ui";
import { useToast } from "../components/Toaster";

const groupKey = (g: DuplicateGroup) => g.items.map((i) => i.id).join(",");

export function Duplicates({ info, onInfoChange, onBack }: { info: AppInfo; onInfoChange: () => void; onBack: () => void }) {
  const toast = useToast();
  const includeSubdomains = info.settings.duplicatesIncludeSubdomains;
  const [groups, setGroups] = useState<DuplicateGroup[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The copy to keep per group; the newest unless the person picks another. */
  const [keep, setKeep] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(() => {
    api
      .findDuplicates(includeSubdomains)
      .then((g) => {
        setGroups(g);
        setError(null);
      })
      .catch((e) => setError(errorText(e)));
  }, [includeSubdomains]);

  useEffect(() => {
    load();
    window.addEventListener("lp-vault-changed", load);
    return () => window.removeEventListener("lp-vault-changed", load);
  }, [load]);

  const setSubdomains = async (on: boolean) => {
    try {
      await api.saveSettings({ ...info.settings, duplicatesIncludeSubdomains: on });
      onInfoChange();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const trashOthers = async (g: DuplicateGroup) => {
    const key = groupKey(g);
    const keptId = keep[key] ?? g.items[0].id;
    const kept = g.items.find((i) => i.id === keptId) ?? g.items[0];
    const others = g.items.filter((i) => i.id !== kept.id);
    const lost = lostWithTrash(kept, others);
    if (lost.length > 0) {
      const yes = await ask(
        `A copy you are about to trash has ${lost.join(", ")} that "${kept.title}" does not. They stay in the Trash until you empty it, but they are not copied over.\n\nMove ${others.length === 1 ? "it" : "them"} to the trash anyway?`,
        { title: "Move to trash", kind: "warning" },
      );
      if (!yes) return;
    }
    setBusy(key);
    try {
      for (const o of others) await api.trashItem(o.id);
      toast(`Moved ${others.length} duplicate${others.length === 1 ? "" : "s"} to the trash`);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
      load();
    }
  };

  return (
    <div className="mx-auto max-w-3xl space-y-4 p-4 md:p-6">
      <div className="flex items-center gap-2">
        <IconButton label="Back to settings" onClick={onBack}>
          <ArrowLeft className="size-5" />
        </IconButton>
        <h1 className="text-xl font-semibold">Duplicate logins</h1>
      </div>
      <p className="text-sm text-slate-600 dark:text-slate-400">
        Logins with the same username and the same password for the same website. Keep one copy and move the others to the trash.
      </p>

      <Card className="px-4 py-2">
        <Toggle
          checked={includeSubdomains}
          onChange={setSubdomains}
          label="Count subdomains as the same site"
          description="Off: a.example.com and example.com are different sites. Either way www. is ignored. Logins without a website are not checked."
        />
      </Card>

      {error && <ErrorText>{error}</ErrorText>}
      {!error && groups === null && <p className="text-sm text-slate-500">Looking…</p>}
      {groups?.length === 0 && (
        <Card className="p-4 text-sm text-slate-600 dark:text-slate-400">No duplicate logins found.</Card>
      )}

      {groups?.map((g) => {
        const key = groupKey(g);
        const keptId = keep[key] ?? g.items[0].id;
        return (
          <Card key={key} className="space-y-3 p-4">
            <div className="space-y-1">
              <div className="truncate font-medium">{g.username || "(no username)"}</div>
              <div className="flex flex-wrap gap-1.5">
                {g.sites.map((s) => (
                  <Badge key={s} tone="indigo">
                    {s}
                  </Badge>
                ))}
              </div>
            </div>
            <ul className="divide-y divide-slate-100 dark:divide-slate-800" role="radiogroup" aria-label="Copy to keep">
              {g.items.map((i) => (
                <li key={i.id}>
                  <label className="flex cursor-pointer items-start gap-3 py-2">
                    <input
                      type="radio"
                      name={key}
                      className="mt-1 size-4 shrink-0 accent-indigo-600"
                      checked={i.id === keptId}
                      onChange={() => setKeep((k) => ({ ...k, [key]: i.id }))}
                    />
                    <span className="min-w-0 flex-1 space-y-0.5">
                      <span className="flex items-center gap-1.5">
                        <span className="truncate font-medium">{i.title}</span>
                        {i.favorite && <Star className="size-3.5 shrink-0 fill-amber-400 text-amber-400" aria-label="Favorite" />}
                        {i.id === keptId && <span className="shrink-0 text-xs font-medium text-indigo-600 dark:text-indigo-400">Keep</span>}
                      </span>
                      <span className="block truncate text-xs text-slate-500 dark:text-slate-400">{i.urls.join(", ")}</span>
                      <span className="block text-xs text-slate-500 dark:text-slate-400">
                        Changed {timeAgo(i.updatedMs)} on {i.updatedBy}
                      </span>
                      <span className="flex flex-wrap gap-1.5 pt-0.5">
                        {i.hasTotp && <Badge tone="green">2FA</Badge>}
                        {i.hasNotes && <Badge>Notes</Badge>}
                        {i.customFields > 0 && <Badge>{i.customFields} custom field{i.customFields === 1 ? "" : "s"}</Badge>}
                        {i.conflicted && <Badge tone="amber">Conflict</Badge>}
                      </span>
                    </span>
                  </label>
                </li>
              ))}
            </ul>
            <Button variant="danger" busy={busy === key} onClick={() => trashOthers(g)}>
              Move the other {g.items.length - 1 === 1 ? "copy" : `${g.items.length - 1} copies`} to trash
            </Button>
          </Card>
        );
      })}
    </div>
  );
}
