import { TorrentStats } from "../api-types";
import { statusClass, statusDetailOf } from "../helper/status";

/** Colored badge with the server-computed torrent status. */
export const StatusBadge: React.FC<{
  stats?: TorrentStats | null;
  className?: string;
}> = ({ stats, className = "" }) => {
  const d = statusDetailOf(stats);
  if (!d) return null;
  const tip = [
    d.label,
    stats?.queue_position ? `Queue position #${stats.queue_position}` : null,
    stats?.error ?? null,
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <span
      className={`inline-block max-w-full truncate rounded px-1.5 py-0.5 text-xs font-medium ${statusClass(d)} ${className}`}
      title={tip}
      data-testid="status-badge"
      data-status={d.kind}
    >
      {d.label}
    </span>
  );
};
