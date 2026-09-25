import { useEffect, useState } from "react";
import { Button } from "../buttons/Button";

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
  /** Epoch ms when the add request for this item was sent. */
  startedAt?: number;
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
}> = ({ items, running, onRemove, onClear, onDismissErrors, onRetry }) => {
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
                  {statusLabel(item.status)}
                  {(item.status === "running" || item.status === "resolving") &&
                    item.startedAt && (
                      <>
                        {" "}
                        <Elapsed since={item.startedAt} />
                      </>
                    )}
                </span>
                {item.matchStatus === "matched" && item.matchedPath && (
                  <span
                    className="text-green-600 dark:text-green-400 truncate"
                    title={item.matchedPath}
                  >
                    matched {Math.round((item.matchConfidence ?? 0) * 100)}% ·{" "}
                    {item.matchedPath.split(/[/\\]/).pop()}
                  </span>
                )}
                {item.matchStatus === "ambiguous" && (
                  <span
                    className="text-amber-600 dark:text-amber-400 truncate"
                    title={item.matchedPath}
                  >
                    ambiguous {Math.round((item.matchConfidence ?? 0) * 100)}%
                    {item.matchedPath
                      ? ` · ${item.matchedPath.split(/[/\\]/).pop()}`
                      : ""}
                  </span>
                )}
                {item.matchStatus === "unmatched" && (
                  <span className="text-secondary">unmatched</span>
                )}
                {item.status === "resolving" && (
                  <span className="text-secondary">
                    waiting for peers to send torrent info
                  </span>
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
            <button
              type="button"
              className="flex-shrink-0 text-secondary hover:text-text px-1 disabled:opacity-40"
              disabled={
                running &&
                (item.status === "running" || item.status === "resolving")
              }
              aria-label="Remove"
              onClick={() => onRemove(item.id)}
            >
              ×
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
};
