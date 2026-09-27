import { useContext, useState, useCallback, useMemo } from "react";
import {
  FaPause,
  FaPlay,
  FaTrash,
  FaRedo,
  FaWrench,
  FaAngleUp,
  FaAngleDown,
  FaAngleDoubleUp,
  FaAngleDoubleDown,
} from "react-icons/fa";
import { GoSearch, GoX } from "react-icons/go";
import debounce from "lodash.debounce";
import { APIContext } from "../../context";
import { useUIStore } from "../../stores/uiStore";
import { useTorrentStore } from "../../stores/torrentStore";
import { useErrorStore } from "../../stores/errorStore";
import { DeleteTorrentModal } from "../modal/DeleteTorrentModal";
import { useKeyboardShortcuts } from "../../hooks/useKeyboardShortcuts";
import {
  ErrorDetails,
  QueueMoveAction,
  STATE_ERROR,
  STATE_LIVE,
  STATE_PAUSED,
  TorrentListItem,
} from "../../api-types";
import { Button } from "../buttons/Button";
import {
  hasDamagedFiles,
  hasRecoveryIssues,
  isRepairRunning,
} from "../../helper/damage";
import {
  StatusFilter,
  STATUS_FILTER_LABELS,
} from "../../helper/torrentFilters";

interface ActionBarProps {
  // When true, hides search/filter/selection count (for use in modal)
  hideFilters?: boolean;
}

export const ActionBar: React.FC<ActionBarProps> = ({ hideFilters }) => {
  const selectedTorrentIds = useUIStore((state) => state.selectedTorrentIds);
  const clearSelection = useUIStore((state) => state.clearSelection);
  const searchQuery = useUIStore((state) => state.searchQuery);
  const setSearchQuery = useUIStore((state) => state.setSearchQuery);
  const statusFilter = useUIStore((state) => state.statusFilter);
  const setStatusFilter = useUIStore((state) => state.setStatusFilter);
  const torrents = useTorrentStore((state) => state.torrents);
  const refreshTorrents = useTorrentStore((state) => state.refreshTorrents);
  const setCloseableError = useErrorStore((state) => state.setCloseableError);

  const [disabled, setDisabled] = useState(false);
  const [showDeleteModal, setShowDeleteModal] = useState(false);
  const [torrentsToDelete, setTorrentsToDelete] = useState<
    Pick<TorrentListItem, "id" | "name">[]
  >([]);
  const [localSearch, setLocalSearch] = useState(searchQuery);

  // Debounced update to store
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const debouncedSetSearch = useCallback(
    debounce((value: string) => setSearchQuery(value), 150),
    [setSearchQuery],
  );

  const handleSearchChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;
    setLocalSearch(value);
    debouncedSetSearch(value);
  };

  const clearSearch = () => {
    setLocalSearch("");
    setSearchQuery("");
  };

  const API = useContext(APIContext);

  const selectedCount = selectedTorrentIds.size;
  const hasSelection = selectedCount > 0;

  const getTorrentById = (id: number) => torrents?.find((t) => t.id === id);

  const openDeleteModal = useCallback(() => {
    // Capture current selection when opening modal (stable snapshot)
    const torrentsList = Array.from(selectedTorrentIds).map((id) => {
      const torrent = getTorrentById(id);
      return {
        id,
        name: torrent?.name ?? null,
      };
    });
    setTorrentsToDelete(torrentsList);
    setShowDeleteModal(true);
  }, [selectedTorrentIds, torrents]);

  // Keyboard shortcuts for compact view
  const keyboardActions = useMemo(
    () => ({ onDelete: openDeleteModal }),
    [openDeleteModal],
  );
  useKeyboardShortcuts(keyboardActions);

  const runBulkAction = async (
    action: (id: number) => Promise<void>,
    skipState: string,
    errorLabel: string,
  ) => {
    setDisabled(true);
    try {
      for (const id of selectedTorrentIds) {
        const torrent = getTorrentById(id);
        if (torrent?.stats?.state === skipState) continue;
        try {
          await action(id);
          refreshTorrents();
        } catch (e) {
          setCloseableError({
            text: `Error ${errorLabel} torrent id=${id}`,
            details: e as ErrorDetails,
          });
        }
      }
      clearSelection();
    } finally {
      setDisabled(false);
    }
  };

  const pauseSelected = () =>
    runBulkAction((id) => API.pause(id), STATE_PAUSED, "pausing");
  const resumeSelected = () =>
    runBulkAction((id) => API.start(id), STATE_LIVE, "starting");
  const restartSelected = () =>
    runBulkAction((id) => API.restart(id), "", "restarting");
  // Fix errors: torrents with damaged files get "Repair damaged files" (which also
  // resumes them); other torrents in error state get the normal soft re-check.
  const fixErrorsSelected = async () => {
    setDisabled(true);
    try {
      for (const id of selectedTorrentIds) {
        const torrent = getTorrentById(id);
        const damaged = hasDamagedFiles(torrent?.stats?.damage);
        if (damaged && isRepairRunning(torrent?.stats?.damage)) continue;
        const held = hasRecoveryIssues(torrent?.stats?.damage);
        if (!damaged && !held && torrent?.stats?.state !== STATE_ERROR)
          continue;
        try {
          if (damaged && API.repairFiles) {
            await API.repairFiles(id);
          } else {
            await API.fixErrors(id);
          }
          refreshTorrents();
        } catch (e) {
          setCloseableError({
            text: `Error fixing errors on torrent id=${id}`,
            details: e as ErrorDetails,
          });
        }
      }
      clearSelection();
    } finally {
      setDisabled(false);
    }
  };

  // Queue order (multi-select keeps relative order). Positions are persisted server-side.
  const moveQueue = async (action: QueueMoveAction) => {
    if (!API.queueMove) return;
    setDisabled(true);
    try {
      await API.queueMove(Array.from(selectedTorrentIds), action);
      refreshTorrents();
    } catch (e) {
      setCloseableError({
        text: `Error moving torrents in queue`,
        details: e as ErrorDetails,
      });
    } finally {
      setDisabled(false);
    }
  };

  const queueButtons: [QueueMoveAction, string, React.ReactNode][] = [
    ["top", "Move to top of queue", <FaAngleDoubleUp className="w-3 h-3" />],
    ["up", "Move up in queue", <FaAngleUp className="w-3 h-3" />],
    ["down", "Move down in queue", <FaAngleDown className="w-3 h-3" />],
    [
      "bottom",
      "Move to bottom of queue",
      <FaAngleDoubleDown className="w-3 h-3" />,
    ],
  ];

  return (
    <div className="flex items-center gap-1.5 px-3 py-1.5 bg-surface-raised border-b border-divider">
      <Button
        onClick={resumeSelected}
        disabled={disabled || !hasSelection}
        variant="secondary"
      >
        <FaPlay className="w-2.5 h-2.5" />
        Resume
      </Button>
      <Button
        onClick={pauseSelected}
        disabled={disabled || !hasSelection}
        variant="secondary"
      >
        <FaPause className="w-2.5 h-2.5" />
        Pause
      </Button>
      <Button
        onClick={restartSelected}
        disabled={disabled || !hasSelection}
        variant="secondary"
      >
        <FaRedo className="w-2.5 h-2.5" />
        Restart
      </Button>
      <Button
        onClick={fixErrorsSelected}
        disabled={disabled || !hasSelection}
        variant="secondary"
      >
        <FaWrench className="w-2.5 h-2.5" />
        Fix errors
      </Button>
      {API.queueMove && (
        <div className="flex items-center gap-0.5" data-testid="queue-buttons">
          {queueButtons.map(([action, title, icon]) => (
            <Button
              key={action}
              onClick={() => moveQueue(action)}
              disabled={disabled || !hasSelection}
              variant="secondary"
              title={title}
            >
              {icon}
            </Button>
          ))}
        </div>
      )}
      <Button
        onClick={openDeleteModal}
        disabled={disabled || !hasSelection}
        variant="danger"
      >
        <FaTrash className="w-2.5 h-2.5" />
        Delete
      </Button>

      {!hideFilters && (
        <>
          {hasSelection && (
            <span className="ml-1.5 text-secondary">
              {selectedCount} selected
            </span>
          )}

          {/* Spacer */}
          <div className="flex-1" />

          {/* Status filter */}
          <select
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value as StatusFilter)}
            className="py-1 px-2 text-sm bg-surface border border-divider rounded focus:outline-none focus:border-primary"
          >
            {(Object.keys(STATUS_FILTER_LABELS) as StatusFilter[]).map(
              (status) => (
                <option key={status} value={status}>
                  {STATUS_FILTER_LABELS[status]}
                </option>
              ),
            )}
          </select>

          {/* Search input */}
          <div className="relative">
            <GoSearch className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-tertiary" />
            <input
              type="text"
              data-search-input
              value={localSearch}
              onChange={handleSearchChange}
              placeholder="Search..."
              className="pl-7 pr-7 py-1 w-48 text-sm bg-surface border border-divider rounded focus:outline-none focus:border-primary placeholder:text-tertiary"
            />
            {localSearch && (
              <button
                onClick={clearSearch}
                className="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 text-tertiary hover:text-secondary rounded cursor-pointer"
              >
                <GoX className="w-3.5 h-3.5" />
              </button>
            )}
          </div>
        </>
      )}

      <DeleteTorrentModal
        show={showDeleteModal}
        onHide={() => setShowDeleteModal(false)}
        torrents={torrentsToDelete}
      />
    </div>
  );
};
