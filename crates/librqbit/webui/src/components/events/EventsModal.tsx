import { useCallback, useContext, useEffect, useRef, useState } from "react";
import { APIContext } from "../../context";
import { EventRecord, EventSeverity } from "../../api-types";
import { useEventsStore } from "../../stores/eventsStore";
import { formatBytes } from "../../helper/formatBytes";
import { Modal } from "../modal/Modal";
import { ModalBody } from "../modal/ModalBody";
import { ModalFooter } from "../modal/ModalFooter";
import { Button } from "../buttons/Button";
import { EventRow } from "./EventRow";
import { KIND_FILTERS } from "./eventHelpers";

const PAGE = 100;

const selectClass =
  "bg-surface border border-divider rounded px-2 py-1 text-sm text-text";

const Counter: React.FC<{
  label: string;
  value: React.ReactNode;
  className?: string;
  title?: string;
}> = ({ label, value, className, title }) => (
  <div className="flex flex-col min-w-24" title={title}>
    <span className="text-xs text-tertiary">{label}</span>
    <span className={`text-lg font-semibold tabular-nums ${className ?? ""}`}>
      {value}
    </span>
  </div>
);

export const EventsModal: React.FC = () => {
  const api = useContext(APIContext);
  const open = useEventsStore((s) => s.open);
  const presetInfoHash = useEventsStore((s) => s.presetInfoHash);
  const closeEvents = useEventsStore((s) => s.closeEvents);
  const summary = useEventsStore((s) => s.summary);
  const refreshSummary = useEventsStore((s) => s.refreshSummary);
  const markSeen = useEventsStore((s) => s.markSeen);

  const [kind, setKind] = useState("");
  const [severity, setSeverity] = useState<"" | EventSeverity>("");
  const [infoHash, setInfoHash] = useState<string | null>(null);
  const [events, setEvents] = useState<EventRecord[]>([]);
  const [nextBefore, setNextBefore] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) setInfoHash(presetInfoHash);
  }, [open, presetInfoHash]);

  const load = useCallback(
    async (before?: number) => {
      if (!api.getEvents) return;
      setLoading(true);
      setError(null);
      try {
        const page = await api.getEvents({
          kind: kind || undefined,
          severity: severity || undefined,
          info_hash: infoHash ?? undefined,
          before_seq: before,
          limit: PAGE,
        });
        setEvents((prev) => (before ? [...prev, ...page.events] : page.events));
        setNextBefore(page.next_before_seq);
        if (!before) markSeen(page.latest_seq);
      } catch (e: any) {
        setError(e?.text ?? String(e));
      } finally {
        setLoading(false);
      }
    },
    [api, kind, severity, infoHash, markSeen],
  );

  const eventsLen = useRef(0);
  eventsLen.current = events.length;

  useEffect(() => {
    if (!open) return;
    load();
    refreshSummary(api);
    const t = setInterval(() => {
      // Auto-refresh the first page (not while older pages are loaded).
      if (eventsLen.current <= PAGE) load();
      refreshSummary(api);
    }, 10000);
    return () => clearInterval(t);
  }, [open, load]);

  const reset = async () => {
    if (!api.resetEventCounters) return;
    if (!window.confirm("Reset repair counters? The event log itself is kept."))
      return;
    await api.resetEventCounters();
    refreshSummary(api);
  };

  const c = summary?.counters;
  const torrentLabel =
    infoHash &&
    (events.find((e) => e.info_hash === infoHash)?.torrent_name ??
      infoHash.slice(0, 12) + "…");

  return (
    <Modal
      isOpen={open}
      onClose={closeEvents}
      title="Events: repairs & errors"
      className="max-w-6xl"
    >
      <ModalBody>
        {c && (
          <div className="border border-divider rounded p-3 mb-3">
            <div className="flex flex-wrap gap-x-6 gap-y-2 items-end">
              <Counter label="Auto repairs" value={c.auto_repairs} />
              <Counter label="Manual repairs" value={c.manual_repairs} />
              <Counter label="Files repaired" value={c.files_repaired} />
              <Counter
                label="Zeroed"
                value={formatBytes(c.bytes_zeroed)}
                title="Unreadable bytes punched out / replaced with zeros"
              />
              <Counter
                label="Re-download"
                value={formatBytes(c.bytes_redownload)}
                title="Requeued pieces × piece length"
              />
              <Counter label="Pieces requeued" value={c.pieces_requeued} />
              <Counter
                label="Give-ups"
                value={c.give_ups}
                className={c.give_ups > 0 ? "text-error" : ""}
                title="Automatic recovery gave up (needs attention)"
              />
              <Counter
                label="Repair failures"
                value={c.repair_failures}
                className={c.repair_failures > 0 ? "text-error" : ""}
              />
              <Counter label="I/O errors" value={c.io_errors} />
              <div className="flex-1" />
              <Button size="sm" variant="cancel" onClick={reset}>
                Reset counters
              </Button>
            </div>
            <div className="text-xs text-tertiary mt-2">
              Since {new Date(c.since).toLocaleString()} · log{" "}
              {formatBytes(summary!.log_bytes)} of{" "}
              {formatBytes(summary!.log_cap_bytes)} ({summary!.log_segments}{" "}
              segment{summary!.log_segments === 1 ? "" : "s"})
            </div>
          </div>
        )}

        <div className="flex flex-wrap gap-2 items-center mb-2">
          <select
            className={selectClass}
            value={kind}
            onChange={(e) => setKind(e.target.value)}
            aria-label="Event type"
          >
            {KIND_FILTERS.map((k) => (
              <option key={k.value} value={k.value}>
                {k.label}
              </option>
            ))}
          </select>
          <select
            className={selectClass}
            value={severity}
            onChange={(e) => setSeverity(e.target.value as any)}
            aria-label="Severity"
          >
            <option value="">All severities</option>
            <option value="warning">Warnings & errors</option>
            <option value="error">Errors only</option>
          </select>
          {infoHash && (
            <span className="text-sm bg-primary/10 text-primary rounded px-2 py-0.5 flex items-center gap-1">
              <span className="truncate max-w-80">{torrentLabel}</span>
              <button
                className="cursor-pointer"
                onClick={() => setInfoHash(null)}
                aria-label="Clear torrent filter"
              >
                ×
              </button>
            </span>
          )}
          <div className="flex-1" />
          <Button size="sm" variant="cancel" onClick={() => load()}>
            Refresh
          </Button>
        </div>

        {error && <div className="text-error text-sm mb-2">{error}</div>}
        <div
          className="border border-divider rounded divide-y divide-divider"
          data-testid="events-list"
        >
          {events.length === 0 && !loading && (
            <div className="p-4 text-center text-tertiary text-sm">
              No events.
            </div>
          )}
          {events.map((e) => (
            <EventRow key={e.seq} event={e} onNavigate={closeEvents} />
          ))}
        </div>
        {nextBefore !== null && (
          <div className="mt-2 text-center">
            <Button
              size="sm"
              variant="cancel"
              disabled={loading}
              onClick={() => load(nextBefore)}
            >
              Load older
            </Button>
          </div>
        )}
      </ModalBody>
      <ModalFooter>
        <Button variant="primary" onClick={closeEvents}>
          Close
        </Button>
      </ModalFooter>
    </Modal>
  );
};
