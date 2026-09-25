import { useCallback, useContext, useEffect, useState } from "react";
import { APIContext } from "../../context";
import { FsEntry, FsRoot } from "../../api-types";
import { Button } from "../buttons/Button";
import { Spinner } from "../Spinner";
import { formatBytes } from "../../helper/formatBytes";

export type FilesystemBrowserMode =
  "select-torrents" | "select-directory" | "select-directories";

export type FilesystemBrowserProps = {
  /** select-torrents: pick .torrent files and/or folders of torrents.
   *  select-directory: pick a single folder (for output path prefs later).
   *  select-directories: multi-select folders (transfer from other client). */
  mode?: FilesystemBrowserMode;
  multi?: boolean;
  initialPath?: string;
  className?: string;
  /** Called when user confirms selection. */
  onConfirm: (paths: string[]) => void;
  /** Optional cancel (e.g. embedded in a panel without footer). */
  onCancel?: () => void;
  confirmLabel?: string;
};

/**
 * Reusable server filesystem navigator (Jellyfin-style).
 * Roots come from GET /fs/roots; listing from GET /fs/list.
 */
export const FilesystemBrowser: React.FC<FilesystemBrowserProps> = ({
  mode = "select-torrents",
  multi = true,
  initialPath,
  className,
  onConfirm,
  onCancel,
  confirmLabel,
}) => {
  const API = useContext(APIContext);
  const [roots, setRoots] = useState<FsRoot[]>([]);
  const [path, setPath] = useState<string>(initialPath ?? "");
  const [parent, setParent] = useState<string | null>(null);
  const [entries, setEntries] = useState<FsEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Selection is kept across navigation (keyed by full path) so folders in
  // different places/roots can be picked together.
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [selectedEntries, setSelectedEntries] = useState<Map<string, FsEntry>>(
    new Map(),
  );
  const [truncated, setTruncated] = useState(false);

  const loadRoots = useCallback(async () => {
    try {
      const r = await API.fsRoots();
      setRoots(r.roots);
      if (!path && r.roots.length > 0) {
        setPath(r.roots[0].path);
      }
      if (r.roots.length === 0) {
        setError("No browse roots available on the server.");
      }
    } catch (e: unknown) {
      const err = e as { text?: string; message?: string };
      setError(err?.text || err?.message || String(e));
    }
  }, [API, path]);

  const loadList = useCallback(
    async (p: string) => {
      if (!p) return;
      setLoading(true);
      setError(null);
      try {
        const r = await API.fsList(p);
        setPath(r.path);
        setParent(r.parent ?? null);
        setEntries(r.entries);
        setTruncated(!!r.truncated);
      } catch (e: unknown) {
        const err = e as { text?: string; message?: string };
        setError(err?.text || err?.message || String(e));
      } finally {
        setLoading(false);
      }
    },
    [API],
  );

  useEffect(() => {
    loadRoots();
  }, [loadRoots]);

  useEffect(() => {
    if (path) loadList(path);
  }, [path, loadList]);

  useEffect(() => {
    setSelectedEntries((prev) => {
      const next = new Map<string, FsEntry>();
      for (const p of selected) {
        const e = prev.get(p) ?? entries.find((x) => x.path === p);
        if (e) next.set(p, e);
      }
      return next;
    });
  }, [selected, entries]);

  const isSelectable = (e: FsEntry) =>
    mode === "select-directory" || mode === "select-directories"
      ? e.is_dir
      : e.is_dir || e.is_torrent;

  const selectAllHere = () => {
    const here = entries.filter(isSelectable).map((e) => e.path);
    setSelected((prev) => new Set([...prev, ...here]));
  };

  const toggle = (entry: FsEntry) => {
    if (mode === "select-directory") {
      if (!entry.is_dir) return;
      setSelected(new Set([entry.path]));
      return;
    }
    if (mode === "select-directories") {
      if (!entry.is_dir) return;
      setSelected((prev) => {
        const next = new Set(prev);
        if (next.has(entry.path)) next.delete(entry.path);
        else next.add(entry.path);
        return next;
      });
      return;
    }
    // torrents mode: files that are torrents, or directories (folder of torrents)
    if (!entry.is_dir && !entry.is_torrent) return;
    setSelected((prev) => {
      const next = new Set(multi ? prev : []);
      if (next.has(entry.path)) next.delete(entry.path);
      else next.add(entry.path);
      return next;
    });
  };

  const confirm = async () => {
    if (mode === "select-directory") {
      const dir = selected.size > 0 ? [...selected][0] : path;
      if (dir) onConfirm([dir]);
      return;
    }
    if (mode === "select-directories") {
      const dirs = selected.size > 0 ? [...selected] : path ? [path] : [];
      if (dirs.length) onConfirm(dirs);
      return;
    }
    // Expand selected folders into torrent paths
    const paths: string[] = [];
    for (const p of selected) {
      const ent = selectedEntries.get(p) ?? entries.find((e) => e.path === p);
      if (ent?.is_dir) {
        try {
          const r = await API.fsList(p, {
            recursive: true,
            torrentsOnly: true,
          });
          for (const e of r.entries) {
            if (e.is_torrent) paths.push(e.path);
          }
        } catch (e: unknown) {
          const err = e as { text?: string; message?: string };
          setError(err?.text || err?.message || String(e));
          return;
        }
      } else {
        paths.push(p);
      }
    }
    onConfirm(paths);
    setSelected(new Set());
  };

  const canConfirm =
    mode === "select-directory" || mode === "select-directories"
      ? true
      : selected.size > 0;

  return (
    <div className={`flex flex-col gap-2 ${className ?? ""}`}>
      {roots.length > 1 && (
        <div className="flex flex-wrap gap-1">
          {roots.map((r) => (
            <Button
              key={r.path}
              size="sm"
              variant={path.startsWith(r.path) ? "primary" : "secondary"}
              onClick={() => setPath(r.path)}
            >
              {r.label}
            </Button>
          ))}
        </div>
      )}

      <div className="flex items-center gap-2 text-sm font-mono break-all border border-divider rounded px-2 py-1 bg-surface">
        <Button
          size="sm"
          variant="secondary"
          disabled={!parent}
          onClick={() => parent && setPath(parent)}
        >
          ↑ Up
        </Button>
        <span className="text-secondary">{path || "…"}</span>
      </div>

      {error && (
        <div className="text-sm text-red-600 dark:text-red-400">{error}</div>
      )}

      <div className="border border-divider rounded max-h-64 overflow-y-auto min-h-40 bg-surface">
        {loading ? (
          <div className="flex justify-center p-6">
            <Spinner />
          </div>
        ) : entries.length === 0 ? (
          <div className="p-4 text-sm text-secondary text-center">
            Empty folder
          </div>
        ) : (
          <ul className="divide-y divide-divider">
            {entries.map((e) => {
              const selectable = isSelectable(e);
              const isSelected = selected.has(e.path);
              return (
                <li
                  key={e.path}
                  className={`flex items-center gap-2 px-2 py-1.5 text-sm ${
                    selectable
                      ? "cursor-pointer hover:bg-primary/10"
                      : "opacity-50"
                  } ${isSelected ? "bg-primary/15" : ""}`}
                  onDoubleClick={() => {
                    if (e.is_dir) setPath(e.path);
                  }}
                  onClick={() => selectable && toggle(e)}
                >
                  {selectable && (
                    <input
                      type="checkbox"
                      checked={isSelected}
                      readOnly
                      className="pointer-events-none"
                    />
                  )}
                  <span className="w-5 text-center">
                    {e.is_dir ? "📁" : e.is_torrent ? "🧲" : "📄"}
                  </span>
                  <span
                    className="grow truncate"
                    onDoubleClick={(ev) => {
                      if (e.is_dir) {
                        ev.stopPropagation();
                        setPath(e.path);
                      }
                    }}
                  >
                    {e.name}
                  </span>
                  {e.partial_of && (
                    <span
                      className="text-xs text-amber-600 dark:text-amber-400 whitespace-nowrap"
                      title={`qBittorrent incomplete file for ${e.partial_of}`}
                    >
                      partial (qBittorrent)
                    </span>
                  )}
                  {!e.is_dir && e.size !== undefined && (
                    <span className="text-xs text-secondary whitespace-nowrap tabular-nums">
                      {formatBytes(e.size)}
                    </span>
                  )}
                  {e.is_dir && (
                    <Button
                      size="sm"
                      variant="none"
                      className="text-primary text-xs"
                      onClick={() => setPath(e.path)}
                    >
                      Open
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {(mode === "select-directories" || mode === "select-torrents") &&
        multi && (
          <div className="flex flex-wrap items-center gap-1 text-xs">
            <Button size="sm" variant="secondary" onClick={selectAllHere}>
              Select all here
            </Button>
            {selected.size > 0 && (
              <Button
                size="sm"
                variant="cancel"
                onClick={() => setSelected(new Set())}
              >
                Clear selection
              </Button>
            )}
            {mode === "select-directories" &&
              [...selected].map((p) => (
                <span
                  key={p}
                  className="font-mono bg-primary/10 text-primary rounded px-1.5 py-0.5"
                  title={p}
                >
                  {p.split(/[/\\]/).filter(Boolean).slice(-2).join("/")}
                </span>
              ))}
          </div>
        )}

      {truncated && (
        <div className="text-sm text-secondary">
          Listing truncated — narrow into a subfolder.
        </div>
      )}

      <div className="flex justify-between items-center gap-2">
        <div className="text-sm text-secondary">
          {mode === "select-directory"
            ? "Double-click folders to navigate. Confirm uses selection or current folder."
            : mode === "select-directories"
              ? `${selected.size} folder${selected.size === 1 ? "" : "s"} selected · confirm current if none`
              : `${selected.size} selected · double-click to open folders`}
        </div>
        <div className="flex gap-2">
          {onCancel && (
            <Button size="sm" variant="cancel" onClick={onCancel}>
              Cancel
            </Button>
          )}
          <Button
            size="sm"
            variant="primary"
            disabled={!canConfirm}
            onClick={confirm}
          >
            {confirmLabel ??
              (mode === "select-directory"
                ? "Select folder"
                : mode === "select-directories"
                  ? "Use folders"
                  : "Add to queue")}
          </Button>
        </div>
      </div>
    </div>
  );
};
