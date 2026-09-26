import { TorrentListItem, STATE_INITIALIZING } from "../../api-types";
import { StatusIcon } from "../StatusIcon";
import { formatBytes } from "../../helper/formatBytes";
import { getCompletionETA } from "../../helper/getCompletionETA";
import { damageShortText } from "../../helper/damage";
import { StatusBadge } from "../StatusBadge";
import { memo } from "react";
import {
  TORRENT_TABLE_CELL_PAD,
  TORRENT_TABLE_GRID,
} from "./torrentTableLayout";

interface TorrentTableRowProps {
  torrent: TorrentListItem;
  isSelected: boolean;
  /** Keyboard cursor. */
  isFocused?: boolean;
  onRowClick: (id: number, e: React.MouseEvent) => void;
  onCheckboxChange: (id: number) => void;
}

const TorrentTableRowUnmemoized: React.FC<TorrentTableRowProps> = ({
  torrent,
  isSelected,
  isFocused = false,
  onRowClick,
  onCheckboxChange,
}) => {
  const stats = torrent.stats;
  const state = stats?.state ?? "";
  const error = stats?.error ?? null;
  const totalBytes = stats?.total_bytes ?? 1;
  const progressBytes = stats?.progress_bytes ?? 0;
  const finished = stats?.finished || false;
  const live = !!stats?.live;
  const damageText = damageShortText(stats?.damage);

  const progressPercentage = error
    ? 100
    : totalBytes === 0
      ? 100
      : Math.round((progressBytes / totalBytes) * 100);

  const downloadSpeed = stats?.live?.download_speed?.human_readable ?? "-";
  const uploadSpeed = stats?.live?.upload_speed?.human_readable ?? "-";
  const uploadedBytes = stats?.live?.snapshot.uploaded_bytes ?? 0;

  const peerStats = stats?.live?.snapshot.peer_stats;
  const peersDisplay = peerStats ? `${peerStats.live}/${peerStats.seen}` : "-";

  const eta = stats ? getCompletionETA(stats) : "-";
  const displayEta = finished ? "Done" : eta;

  const name = torrent.name ?? "";

  const handleRowClick = (e: React.MouseEvent) => {
    onRowClick(torrent.id, e);
  };

  const handleCheckboxClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onCheckboxChange(torrent.id);
  };

  const cellBase = `${TORRENT_TABLE_CELL_PAD} min-w-0`;
  const numericCell = `${cellBase} text-right text-secondary whitespace-nowrap`;
  const centeredCell = `${cellBase} text-center text-secondary whitespace-nowrap`;

  return (
    <div
      role="row"
      onMouseDown={handleRowClick}
      aria-selected={isSelected}
      className={`${TORRENT_TABLE_GRID} cursor-pointer border-b border-divider text-sm h-8 ${
        isSelected ? "bg-primary/10" : "hover:bg-surface-raised"
      } ${isFocused ? "ring-1 ring-inset ring-primary" : ""}`}
    >
      <div
        role="gridcell"
        className={`${cellBase} text-center`}
        onMouseDown={handleCheckboxClick}
      >
        <input
          type="checkbox"
          checked={isSelected}
          onChange={() => {}}
          className="w-4 h-4 rounded border-divider-strong bg-surface text-primary focus:ring-primary"
        />
      </div>
      <div
        role="gridcell"
        className="px-1 min-w-0 flex items-center justify-center"
      >
        <StatusIcon
          className="w-5 h-5"
          error={!!error}
          live={live}
          finished={finished}
        />
      </div>
      <div
        role="gridcell"
        className={`${cellBase} text-center text-tertiary font-mono whitespace-nowrap`}
      >
        {torrent.id}
      </div>
      <div
        role="gridcell"
        className={`${cellBase} text-center text-tertiary whitespace-nowrap`}
        data-testid="queue-position"
      >
        {stats?.queue_position ?? ""}
      </div>
      <div role="gridcell" className={cellBase}>
        <div className="truncate" title={name}>
          {name || "Loading..."}
        </div>
        {error && (
          <div className="truncate text-sm text-error" title={error}>
            {error}
          </div>
        )}
        {damageText && (
          <div
            className="truncate text-sm text-warning"
            title={damageText}
            data-testid="damage-text"
          >
            {damageText}
          </div>
        )}
      </div>
      <div role="gridcell" className={cellBase}>
        <StatusBadge stats={stats} compact />
      </div>
      <div role="gridcell" className={numericCell}>
        {formatBytes(totalBytes)}
      </div>
      <div role="gridcell" className={`${cellBase} text-center`}>
        <div className="flex items-center gap-2">
          <div className="flex-1 h-1.5 bg-divider rounded-full overflow-hidden">
            <div
              className={`h-full rounded-full ${
                error
                  ? "bg-error-bg"
                  : finished
                    ? "bg-success-bg"
                    : state === STATE_INITIALIZING
                      ? "bg-warning-bg"
                      : "bg-primary-bg"
              }`}
              style={{ width: `${progressPercentage}%` }}
            />
          </div>
          <span className="text-sm text-secondary w-8 text-right shrink-0">
            {progressPercentage}%
          </span>
        </div>
      </div>
      <div role="gridcell" className={numericCell}>
        {downloadSpeed !== "-" ? (
          <>
            <span className="text-success">↓</span> {downloadSpeed}
          </>
        ) : (
          downloadSpeed
        )}
      </div>
      <div role="gridcell" className={numericCell}>
        {uploadSpeed !== "-" ? (
          <>
            <span className="text-primary">↑</span> {uploadSpeed}
          </>
        ) : (
          uploadSpeed
        )}
      </div>
      <div role="gridcell" className={numericCell}>
        {formatBytes(progressBytes)}
      </div>
      <div role="gridcell" className={numericCell}>
        {uploadedBytes > 0 && <>{formatBytes(uploadedBytes)}</>}
      </div>
      <div role="gridcell" className={centeredCell}>
        {displayEta}
      </div>
      <div
        role="gridcell"
        className={`${cellBase} text-center text-secondary whitespace-nowrap`}
      >
        {peersDisplay}
      </div>
    </div>
  );
};

export const TorrentTableRow = memo(TorrentTableRowUnmemoized);
