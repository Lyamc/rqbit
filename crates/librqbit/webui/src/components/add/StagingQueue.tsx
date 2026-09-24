import { Button } from "../buttons/Button";

export type StagingStatus =
  "pending" | "ready" | "error" | "running" | "ok" | "cancelled";

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
};

const statusLabel = (s: StagingStatus) => {
  switch (s) {
    case "pending":
    case "ready":
      return "ready";
    case "error":
      return "error";
    case "running":
      return "adding…";
    case "ok":
      return "ok";
    case "cancelled":
      return "cancelled";
  }
};

const statusClass = (s: StagingStatus) => {
  switch (s) {
    case "error":
      return "text-red-600 dark:text-red-400";
    case "ok":
      return "text-green-600 dark:text-green-400";
    case "running":
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
}> = ({ items, running, onRemove, onClear, onDismissErrors }) => {
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
                {item.error && (
                  <span className="text-red-600 dark:text-red-400 break-all">
                    {item.error}
                  </span>
                )}
              </div>
            </div>
            <button
              type="button"
              className="flex-shrink-0 text-secondary hover:text-text px-1 disabled:opacity-40"
              disabled={running && item.status === "running"}
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
