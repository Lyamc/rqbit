import { useState } from "react";
import { BsChevronDown, BsChevronRight } from "react-icons/bs";
import { EventRecord } from "../../api-types";
import {
  KIND_LABELS,
  formatEventTime,
  severityDot,
  severityText,
  useOpenTorrent,
  useResolveTorrentId,
} from "./eventHelpers";

export const EventRow: React.FC<{
  event: EventRecord;
  /** Called after a torrent link was followed (e.g. to close the modal). */
  onNavigate?: () => void;
  compact?: boolean;
}> = ({ event: e, onNavigate, compact }) => {
  const [expanded, setExpanded] = useState(false);
  const resolve = useResolveTorrentId();
  const openTorrent = useOpenTorrent();
  const tid = resolve(e);
  const count = e.count ?? 1;
  const hasDetails =
    e.details !== undefined && e.details !== null || !!e.path || !!e.info_hash;

  return (
    <div className="text-sm" data-testid="event-row" data-kind={e.kind} data-severity={e.severity}>
      <div
        className={`flex items-start gap-2 px-2 py-1.5 ${hasDetails ? "cursor-pointer hover:bg-surface-sunken" : ""}`}
        onClick={() => hasDetails && setExpanded(!expanded)}
      >
        <span className="text-tertiary w-3 pt-1 shrink-0">
          {hasDetails &&
            (expanded ? (
              <BsChevronDown className="w-3 h-3" />
            ) : (
              <BsChevronRight className="w-3 h-3" />
            ))}
        </span>
        <span
          className={`w-2 h-2 rounded-full mt-1.5 shrink-0 ${severityDot[e.severity]}`}
          title={e.severity}
        />
        <span
          className="text-tertiary tabular-nums shrink-0 w-20"
          title={new Date(e.time).toLocaleString()}
        >
          {formatEventTime(e.time)}
        </span>
        <span className={`shrink-0 w-28 truncate ${severityText[e.severity]}`}>
          {KIND_LABELS[e.kind] ?? e.kind}
        </span>
        <span className="min-w-0 flex-1">
          {!compact && e.torrent_name && (
            <>
              {tid !== null ? (
                <a
                  href="#"
                  className="text-primary hover:underline"
                  onClick={(ev) => {
                    ev.preventDefault();
                    ev.stopPropagation();
                    openTorrent(tid);
                    onNavigate?.();
                  }}
                  title="Open torrent details"
                >
                  {e.torrent_name}
                </a>
              ) : (
                <span className="text-secondary">{e.torrent_name}</span>
              )}
              <span className="text-tertiary"> · </span>
            </>
          )}
          <span className="break-words">{e.message}</span>
          {count > 1 && (
            <span className="ms-2 text-xs bg-error/10 text-error rounded px-1.5 py-0.5">
              ×{count}
            </span>
          )}
        </span>
      </div>
      {expanded && (
        <div className="px-2 pb-2 ps-10">
          {e.path && (
            <div className="text-xs text-secondary font-mono break-all mb-1">
              {e.path}
            </div>
          )}
          <pre className="text-xs bg-surface-sunken rounded p-2 overflow-x-auto max-h-80">
            {JSON.stringify(
              {
                seq: e.seq,
                time: e.time,
                kind: e.kind,
                torrent_id: e.torrent_id,
                info_hash: e.info_hash,
                file_id: e.file_id,
                count: e.count,
                ...(e.details ? { details: e.details } : {}),
              },
              null,
              2,
            )}
          </pre>
        </div>
      )}
    </div>
  );
};
