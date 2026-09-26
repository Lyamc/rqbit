import {
  useMemo,
  useCallback,
  useEffect,
  useState,
  useRef,
  forwardRef,
} from "react";
import { Virtuoso, VirtuosoHandle } from "react-virtuoso";
import { TorrentListItem } from "../../api-types";
import { TorrentTableRow } from "./TorrentTableRow";
import { useUIStore } from "../../stores/uiStore";
import { Spinner } from "../Spinner";
import { TableHeader } from "./TableHeader";
import { statusSortValue } from "../../helper/status";
import { isTorrentVisible, SortDirection } from "../../helper/torrentFilters";
import { Nav } from "../../helper/selection";
import {
  TORRENT_TABLE_CELL_PAD,
  TORRENT_TABLE_GRID,
} from "./torrentTableLayout";

/** Virtuoso scroller with stable gutter so header/body column widths match. */
const GutterScroller = forwardRef<
  HTMLDivElement,
  React.HTMLAttributes<HTMLDivElement>
>(function GutterScroller({ style, ...props }, ref) {
  return (
    <div {...props} ref={ref} style={{ ...style, scrollbarGutter: "stable" }} />
  );
});

// Extended sort columns for table view (includes columns not in card view)
export type TableSortColumn =
  | "id"
  | "name"
  | "size"
  | "progress"
  | "downloadedBytes"
  | "downSpeed"
  | "upSpeed"
  | "uploadedBytes"
  | "eta"
  | "peers"
  | "queue"
  | "status";

const DEFAULT_SORT_COLUMN: TableSortColumn = "id";
const DEFAULT_SORT_DIRECTION: SortDirection = "desc";

function getTableSortValue(
  t: TorrentListItem,
  column: TableSortColumn,
): number | string {
  switch (column) {
    case "id":
      return t.id;
    case "name":
      return (t.name ?? "").toLowerCase();
    case "size":
      return t.stats?.total_bytes ?? 0;
    case "progress":
      return t.stats?.total_bytes
        ? (t.stats.progress_bytes ?? 0) / t.stats.total_bytes
        : 0;
    case "downloadedBytes":
      return t.stats?.progress_bytes ?? 0;
    case "downSpeed":
      return t.stats?.live?.download_speed?.mbps ?? 0;
    case "upSpeed":
      return t.stats?.live?.upload_speed?.mbps ?? 0;
    case "uploadedBytes":
      return t.stats?.live?.snapshot.uploaded_bytes ?? 0;
    case "eta": {
      if (!t.stats?.live) return Infinity;
      const remaining =
        (t.stats.total_bytes ?? 0) - (t.stats.progress_bytes ?? 0);
      const speed = t.stats.live.download_speed?.mbps ?? 0;
      if (speed <= 0 || remaining <= 0) return remaining <= 0 ? 0 : Infinity;
      return remaining / (speed * 1024 * 1024);
    }
    case "peers":
      return t.stats?.live?.snapshot.peer_stats?.live ?? 0;
    case "queue":
      return t.stats?.queue_position ?? Infinity;
    case "status":
      return statusSortValue(t.stats);
  }
}

interface TorrentTableProps {
  torrents: TorrentListItem[] | null;
  loading: boolean;
}

export const TorrentTable: React.FC<TorrentTableProps> = ({
  torrents,
  loading,
}) => {
  const selectedTorrentIds = useUIStore((state) => state.selectedTorrentIds);
  const selectTorrent = useUIStore((state) => state.selectTorrent);
  const toggleSelection = useUIStore((state) => state.toggleSelection);
  const selectRange = useUIStore((state) => state.selectRange);
  const focusedTorrentId = useUIStore((state) => state.focusedTorrentId);
  const navigateSelection = useUIStore((state) => state.navigateSelection);
  const toggleFocusedSelection = useUIStore(
    (state) => state.toggleFocusedSelection,
  );
  const selectAll = useUIStore((state) => state.selectAll);
  const clearSelection = useUIStore((state) => state.clearSelection);
  const searchQuery = useUIStore((state) => state.searchQuery);
  const statusFilter = useUIStore((state) => state.statusFilter);

  const normalizedQuery = searchQuery.toLowerCase().trim();

  const [sortColumn, setSortColumnState] =
    useState<TableSortColumn>(DEFAULT_SORT_COLUMN);
  const [sortDirection, setSortDirectionState] = useState<SortDirection>(
    DEFAULT_SORT_DIRECTION,
  );

  const setSortColumn = useCallback((column: TableSortColumn) => {
    setSortColumnState((prevColumn) => {
      setSortDirectionState((prevDir) => {
        const newDir: SortDirection =
          prevColumn === column ? (prevDir === "asc" ? "desc" : "asc") : "desc";
        return newDir;
      });
      return column;
    });
  }, []);

  const filteredTorrents = useMemo(() => {
    if (!torrents) return null;

    return [...torrents]
      .filter((t) => isTorrentVisible(t, normalizedQuery, statusFilter))
      .sort((a, b) => {
        const aVal = getTableSortValue(a, sortColumn);
        const bVal = getTableSortValue(b, sortColumn);
        const cmp =
          typeof aVal === "string"
            ? aVal.localeCompare(bVal as string)
            : (aVal as number) - (bVal as number);
        return sortDirection === "asc" ? cmp : -cmp;
      });
  }, [torrents, normalizedQuery, statusFilter, sortColumn, sortDirection]);

  const visibleTorrentIds = useMemo(() => {
    if (!filteredTorrents) return [];
    return filteredTorrents.map((t) => t.id);
  }, [filteredTorrents]);

  const allSelected = !!(
    visibleTorrentIds.length > 0 &&
    visibleTorrentIds.every((id) => selectedTorrentIds.has(id))
  );
  const someSelected = visibleTorrentIds.some((id) =>
    selectedTorrentIds.has(id),
  );

  const handleHeaderCheckbox = () => {
    if (allSelected) {
      clearSelection();
    } else {
      selectAll(visibleTorrentIds);
    }
  };

  const handleSort = (column: TableSortColumn) => {
    setSortColumn(column);
  };

  const orderedIdsRef = useRef<number[]>([]);
  orderedIdsRef.current = visibleTorrentIds;
  const virtuosoRef = useRef<VirtuosoHandle>(null);

  // Keyboard: arrows/Home/End move the cursor (shift extends the range,
  // ctrl just moves), Space toggles the cursor row, Enter shows its details.
  // Ctrl+A / Esc / Delete live in useKeyboardShortcuts.
  useEffect(() => {
    const NAV_KEYS: Record<string, Nav> = {
      ArrowUp: "up",
      ArrowDown: "down",
      Home: "home",
      End: "end",
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.altKey) return;
      const el = (e.target as HTMLElement | null) ?? document.activeElement;
      if (
        el instanceof HTMLElement &&
        (el.tagName === "INPUT" ||
          el.tagName === "TEXTAREA" ||
          el.tagName === "SELECT" ||
          el.isContentEditable)
      ) {
        return;
      }
      if (document.querySelector('[role="dialog"]')) return;
      const ordered = orderedIdsRef.current;
      const nav = NAV_KEYS[e.key];
      if (nav) {
        e.preventDefault();
        const index = navigateSelection(nav, e.shiftKey, ordered);
        if (index >= 0) {
          virtuosoRef.current?.scrollIntoView({ index, behavior: "auto" });
        }
        return;
      }
      // Space/Enter on a focused button or link keep their normal meaning.
      const onControl =
        el instanceof HTMLElement &&
        (el.tagName === "BUTTON" ||
          el.tagName === "A" ||
          !!el.closest("button"));
      if (e.key === " " && !e.shiftKey && !onControl) {
        e.preventDefault();
        toggleFocusedSelection(ordered);
        return;
      }
      if (e.key === "Enter" && !onControl && !e.shiftKey) {
        const { focusedTorrentId, selectedTorrentIds } = useUIStore.getState();
        const target =
          focusedTorrentId !== null && ordered.includes(focusedTorrentId)
            ? focusedTorrentId
            : selectedTorrentIds.size === 1
              ? [...selectedTorrentIds][0]
              : null;
        if (target !== null) {
          e.preventDefault();
          // The details pane follows a single selection.
          selectTorrent(target);
        }
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [navigateSelection, toggleFocusedSelection, selectTorrent]);

  const handleRowClick = useCallback(
    (id: number, e: React.MouseEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (e.shiftKey) {
        // No text selection on shift-click; move focus off inputs so the
        // keyboard keeps working on the list.
        e.preventDefault();
        window.getSelection()?.removeAllRanges();
        const active = document.activeElement;
        if (active instanceof HTMLElement && active !== document.body) {
          active.blur();
        }
        selectRange(id, orderedIdsRef.current, mod);
      } else if (mod) {
        e.preventDefault();
        toggleSelection(id);
      } else {
        selectTorrent(id);
      }
    },
    [selectRange, selectTorrent, toggleSelection],
  );

  const itemContent = useCallback(
    (index: number) => {
      const torrent = filteredTorrents![index];
      return (
        <TorrentTableRow
          key={torrent.id}
          torrent={torrent}
          isSelected={selectedTorrentIds.has(torrent.id)}
          isFocused={focusedTorrentId === torrent.id}
          onRowClick={handleRowClick}
          onCheckboxChange={toggleSelection}
        />
      );
    },
    [
      filteredTorrents,
      selectedTorrentIds,
      focusedTorrentId,
      handleRowClick,
      toggleSelection,
    ],
  );

  if (loading) {
    return (
      <div className="flex justify-center items-center h-64">
        <Spinner />
      </div>
    );
  }

  if (!torrents || torrents.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-64 text-tertiary">
        <p className="text-lg">No torrents</p>
        <p className="">Add a torrent to get started</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full" role="grid">
      {/* Header: same grid template as rows; gutter matches Virtuoso scroller */}
      <div className="shrink-0 bg-surface-raised text-sm overflow-y-auto [scrollbar-gutter:stable]">
        <div
          role="row"
          className={`${TORRENT_TABLE_GRID} border-b border-divider`}
        >
          <div
            role="columnheader"
            className={`${TORRENT_TABLE_CELL_PAD} py-3 text-center`}
          >
            <input
              type="checkbox"
              checked={allSelected}
              ref={(el) => {
                if (el) el.indeterminate = someSelected && !allSelected;
              }}
              onChange={handleHeaderCheckbox}
              className="w-4 h-4 rounded border-divider-strong bg-surface text-primary focus:ring-primary"
            />
          </div>
          <div role="columnheader" className="px-1 py-3" />
          <TableHeader
            column="id"
            label="ID"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="center"
          />
          <TableHeader
            column="queue"
            label="#"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="center"
          />
          <TableHeader
            column="name"
            label="Name"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="left"
          />
          <TableHeader
            column="status"
            label="Status"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="left"
          />
          <TableHeader
            column="size"
            label="Size"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="right"
          />
          <TableHeader
            column="progress"
            label="Progress"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="center"
          />
          <TableHeader
            column="downSpeed"
            label="↓ Download"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="right"
          />
          <TableHeader
            column="upSpeed"
            label="↑ Upload"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="right"
          />
          <TableHeader
            column="downloadedBytes"
            label="Received"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="right"
          />
          <TableHeader
            column="uploadedBytes"
            label="Sent"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="right"
          />
          <TableHeader
            column="eta"
            label="ETA"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="center"
          />
          <TableHeader
            column="peers"
            label="Peers"
            sortColumn={sortColumn}
            sortDirection={sortDirection}
            onSort={handleSort}
            align="center"
          />
        </div>
      </div>
      <div className="flex-1 min-h-0">
        <Virtuoso
          ref={virtuosoRef}
          totalCount={filteredTorrents?.length ?? 0}
          itemContent={itemContent}
          style={{ height: "100%" }}
          components={{ Scroller: GutterScroller }}
        />
      </div>
    </div>
  );
};
