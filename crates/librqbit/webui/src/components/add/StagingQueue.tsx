import { useEffect, useState } from "react";
import { Button } from "../buttons/Button";
import { AddJobStage } from "../../api-types";

export type StagingStatus =
  "pending" | "ready" | "error" | "running" | "resolving" | "ok" | "cancelled";

export type StagingKind =
  "magnet" | "url" | "file" | "server_path" | "torrent_bytes";

export type TransferMatchStatus = "matched" | "unmatched" | "ambiguous";

export type StagingItem = {
  id: string;
  label: string;
  source: string;
  status: StagingStatus;
  error?: string;
  kind: StagingKind;
  /** magnet / http URL string */
  text?: string;
  file?: File;
  serverPath?: string;
  /** raw torrent bytes (from zip extract) */
  bytes?: Uint8Array;
  /** Transfer-from-other-client match. */
  matchedPath?: string;
  matchStatus?: TransferMatchStatus;
  matchConfidence?: number;
  matchReason?: string;
  /** Folder/file the match was based on. */
  matchEntryPath?: string;
  /** Close alternatives when ambiguous (entry paths). */
  matchAlternates?: string[];
  /** Torrent name + files, read via list_only (transfer matching). */
  meta?: {
    name?: string;
    files: { components: string[]; length: number }[];
  };
  /** Epoch ms when the add request for this item was sent. */
  startedAt?: number;
  /** Server-reported stage of the in-flight add (GET /add_jobs/{id}). */
  serverStage?: AddJobStage;
  /** Epoch ms when the server entered `serverStage`. */
  stageSince?: number;
  /** Set when the torrent ended up in rqbit (e.g. a cancel came too late). */
  addedTorrentId?: number;
  /** Informational note (not an error). */
  note?: string;
  /** User asked to cancel; waiting for the server's answer. */
  cancelling?: boolean;
};

/** In-flight label from what the server reports it is doing. */
const stageLabel = (st: AddJobStage | undefined): string => {
  switch (st) {
    case "fetching_torrent":
      return "downloading .torrent…";
    case "resolving_metadata":
      return "resolving metadata…";
    case "adopting":
      return "checking existing files…";
    case "waiting_for_server":
      return "waiting for server (busy checking other torrents)…";
    case "adding":
      return "adding (creating files, saving)…";
    case "added":
    case "already_managed":
      return "added, finishing…";
    default:
      return "sending to server…";
  }
};

const itemLabel = (item: StagingItem) => {
  if (item.status === "running" || item.status === "resolving") {
    return item.cancelling ? "cancelling…" : stageLabel(item.serverStage);
  }
  return statusLabel(item.status);
};

const statusLabel = (s: StagingStatus) => {
  switch (s) {
    case "pending":
      return "queued";
    case "ready":
      return "ready";
    case "error":
      return "failed";
    case "running":
      return "adding…";
    case "resolving":
      return "resolving metadata…";
    case "ok":
      return "added";
    case "cancelled":
      return "cancelled";
  }
};

/** Last two path components, e.g. `Complete/Movie (2020)`. */
const shortPath = (p: string) =>
  p.split(/[/\\]/).filter(Boolean).slice(-2).join("/");

const formatElapsed = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

/** Live "m:ss" since `since`, ticking every second. */
const Elapsed: React.FC<{ since: number }> = ({ since }) => {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);
  return <span className="tabular-nums">{formatElapsed(now - since)}</span>;
};

const statusClass = (s: StagingStatus) => {
  switch (s) {
    case "error":
      return "text-red-600 dark:text-red-400";
    case "ok":
      return "text-green-600 dark:text-green-400";
    case "running":
    case "resolving":
      return "text-primary";
    case "cancelled":
      return "text-secondary";
    default:
      return "text-secondary";
  }
};

export const StagingQueue: React.FC<{
  items: StagingItem[];
  running: boolean;
  onRemove: (id: string) => void;
  onClear: () => void;
  onDismissErrors?: () => void;
  onRetry?: (id: string) => void;
  /** Cancel one in-flight add. */
  onCancel?: (id: string) => void;
  /** Remove a torrent that got added (e.g. cancel came too late). */
  onRemoveFromRqbit?: (id: string) => void;
}> = ({
  items,
  running,
  onRemove,
  onClear,
  onDismissErrors,
  onRetry,
  onCancel,
  onRemoveFromRqbit,
}) => {
  const errorCount = items.filter((i) => i.status === "error").length;
  const readyCount = items.filter(
    (i) => i.status === "ready" || i.status === "pending",
  ).length;

  if (items.length === 0) {
    return null;
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <div className="text-sm font-medium">
          Queue · {items.length} item{items.length === 1 ? "" : "s"}
          {readyCount > 0 ? ` · ${readyCount} ready` : ""}
          {errorCount > 0 ? ` · ${errorCount} error` : ""}
        </div>
        <div className="flex gap-1">
          {errorCount > 0 && onDismissErrors && (
            <Button
              size="sm"
              variant="cancel"
              disabled={running}
              onClick={onDismissErrors}
            >
              Dismiss errors
            </Button>
          )}
          <Button
            size="sm"
            variant="cancel"
            disabled={running}
            onClick={onClear}
          >
            Clear all
          </Button>
        </div>
      </div>
      <ul className="border border-divider rounded max-h-56 overflow-y-auto divide-y divide-divider">
        {items.map((item) => (
          <li
            key={item.id}
            className="flex items-start gap-2 px-2 py-1.5 text-sm"
          >
            <div className="grow min-w-0">
              <div className="font-mono truncate" title={item.label}>
                {item.label}
              </div>
              <div className="text-xs text-secondary flex flex-wrap gap-x-2">
                <span>{item.source}</span>
                <span className={statusClass(item.status)}>
                  {itemLabel(item)}
                  {(item.status === "running" || item.status === "resolving") &&
                    (item.stageSince ?? item.startedAt) && (
                      <>
                        {" "}
                        <Elapsed since={item.stageSince ?? item.startedAt!} />
                      </>
                    )}
                </span>
                {item.matchStatus === "matched" && item.matchedPath && (
                  <span
                    className="text-green-600 dark:text-green-400 truncate max-w-full"
                    title={`${item.matchEntryPath ?? item.matchedPath}\n→ output folder: ${item.matchedPath}${item.matchReason ? `\n${item.matchReason}` : ""}`}
                  >
                    matched {Math.round((item.matchConfidence ?? 0) * 100)}% ·{" "}
                    {shortPath(item.matchEntryPath ?? item.matchedPath)}
                    {item.matchReason ? ` · ${item.matchReason}` : ""}
                  </span>
                )}
                {item.matchStatus === "ambiguous" && (
                  <span
                    className="text-amber-600 dark:text-amber-400 truncate max-w-full"
                    title={(item.matchAlternates ?? []).join("\n")}
                  >
                    ambiguous {Math.round((item.matchConfidence ?? 0) * 100)}% ·
                    not added ·{" "}
                    {(item.matchAlternates ?? [])
                      .slice(0, 3)
                      .map(shortPath)
                      .join(" | ")}
                  </span>
                )}
                {item.matchStatus === "unmatched" && (
                  <span
                    className="text-secondary"
                    title={item.matchReason ?? undefined}
                  >
                    unmatched · not added
                    {item.matchReason ? ` · ${item.matchReason}` : ""}
                  </span>
                )}
                {(item.status === "running" || item.status === "resolving") &&
                  !item.cancelling &&
                  item.serverStage === "resolving_metadata" && (
                    <span className="text-secondary">
                      waiting for peers to send torrent info
                    </span>
                  )}
                {item.note && (
                  <span className="text-amber-600 dark:text-amber-400 break-words">
                    {item.note}
                  </span>
                )}
                {item.addedTorrentId !== undefined && onRemoveFromRqbit && (
                  <button
                    type="button"
                    className="text-primary hover:underline cursor-pointer"
                    onClick={() => onRemoveFromRqbit(item.id)}
                  >
                    remove from rqbit (keep files)
                  </button>
                )}
                {item.error && (
                  <span className="text-red-600 dark:text-red-400 break-words">
                    {item.error}
                  </span>
                )}
                {(item.status === "error" || item.status === "cancelled") &&
                  onRetry &&
                  !running && (
                    <button
                      type="button"
                      className="text-primary hover:underline cursor-pointer"
                      onClick={() => onRetry(item.id)}
                    >
                      retry
                    </button>
                  )}
              </div>
            </div>
            {item.status === "running" || item.status === "resolving" ? (
              <button
                type="button"
                className="flex-shrink-0 text-secondary hover:text-text px-1 disabled:opacity-40"
                disabled={!onCancel || item.cancelling}
                aria-label="Cancel"
                title="Cancel this add"
                onClick={() => onCancel?.(item.id)}
              >
                ×
              </button>
            ) : (
              <button
                type="button"
                className="flex-shrink-0 text-secondary hover:text-text px-1 disabled:opacity-40"
                aria-label="Remove"
                onClick={() => onRemove(item.id)}
              >
                ×
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
};
