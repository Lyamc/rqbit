import { EventRecord, EventSeverity } from "../../api-types";
import { useTorrentStore } from "../../stores/torrentStore";
import { useUIStore } from "../../stores/uiStore";

export const KIND_LABELS: Record<string, string> = {
  repair_run: "Repair",
  repair_file: "File repair",
  piece_retry_scheduled: "Retry",
  needs_attention: "Needs attention",
  io_error: "I/O error",
  damage_detected: "Damaged",
  adoption: "Adoption",
  torrent_error: "Torrent error",
  recheck: "Recheck",
};

export const KIND_FILTERS: { label: string; value: string }[] = [
  { label: "All types", value: "" },
  { label: "Repairs", value: "repair_run,repair_file" },
  { label: "Repair runs", value: "repair_run" },
  { label: "Errors", value: "io_error,torrent_error" },
  {
    label: "Recovery",
    value: "damage_detected,piece_retry_scheduled,needs_attention",
  },
  { label: "Needs attention", value: "needs_attention" },
  { label: "Adoption", value: "adoption" },
];

export const severityText: Record<EventSeverity, string> = {
  info: "text-secondary",
  warning: "text-warning",
  error: "text-error",
};

export const severityDot: Record<EventSeverity, string> = {
  info: "bg-tertiary",
  warning: "bg-warning",
  error: "bg-error",
};

export function formatEventTime(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })
    : d.toLocaleString([], {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
}

/** Current torrent id for an event (ids can change across restarts; match by hash). */
export function useResolveTorrentId(): (e: EventRecord) => number | null {
  const torrents = useTorrentStore((s) => s.torrents);
  return (e: EventRecord) => {
    if (e.info_hash && torrents) {
      const t = torrents.find((t) => t.info_hash === e.info_hash);
      if (t) return t.id;
      return null;
    }
    return e.torrent_id ?? null;
  };
}

/** Open a torrent's details (details pane in compact view, modal otherwise). */
export function useOpenTorrent(): (id: number) => void {
  const viewMode = useUIStore((s) => s.viewMode);
  const selectTorrent = useUIStore((s) => s.selectTorrent);
  const openDetailsModal = useUIStore((s) => s.openDetailsModal);
  return (id: number) => {
    const compact = viewMode === "compact" && window.innerWidth >= 1024;
    if (compact) selectTorrent(id);
    else openDetailsModal(id);
  };
}
