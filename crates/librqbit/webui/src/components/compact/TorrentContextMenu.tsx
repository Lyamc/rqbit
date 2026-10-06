import { useContext, useEffect, useState } from "react";
import {
  DownloadOrderPatch,
  DownloadOrderView,
  ErrorDetails,
  QueueMoveAction,
  STATE_ERROR,
  STATE_LIVE,
  STATE_PAUSED,
  TorrentListItem,
} from "../../api-types";
import { APIContext } from "../../context";
import { useTorrentStore } from "../../stores/torrentStore";
import { useUIStore } from "../../stores/uiStore";
import { useErrorStore } from "../../stores/errorStore";
import { ContextMenu, MenuItem } from "../ContextMenu";
import { DeleteTorrentModal } from "../modal/DeleteTorrentModal";
import { torrentOrderMenu } from "../../helper/downloadOrderMenu";
import {
  hasDamagedFiles,
  hasRecoveryIssues,
  isRepairRunning,
} from "../../helper/damage";
import { saveFolderOf } from "../../helper/savePath";

/**
 * Right-click menu for torrent rows. Acts on `ids` (the selection at the time
 * of the right-click; right-clicking an unselected row selects just it).
 */
export const TorrentContextMenu: React.FC<{
  x: number;
  y: number;
  ids: number[];
  onClose: () => void;
}> = ({ x, y, ids, onClose }) => {
  const API = useContext(APIContext);
  const torrents = useTorrentStore((s) => s.torrents);
  const refreshTorrents = useTorrentStore((s) => s.refreshTorrents);
  const openDetailsModal = useUIStore((s) => s.openDetailsModal);
  const setCloseableError = useErrorStore((s) => s.setCloseableError);
  const [order, setOrder] = useState<DownloadOrderView | null>(null);
  const [removing, setRemoving] = useState<
    Pick<TorrentListItem, "id" | "name">[] | null
  >(null);
  const [menuOpen, setMenuOpen] = useState(true);

  const byId = (id: number) => torrents?.find((t) => t.id === id);
  const selected = ids.map(byId).filter((t): t is TorrentListItem => !!t);
  const states = selected.map((t) => t.stats?.state ?? "");
  const single = ids.length === 1 ? ids[0] : null;

  // Current download order for a single torrent (marks the menu).
  useEffect(() => {
    if (single == null || !API.getDownloadOrder) return;
    API.getDownloadOrder(single).then(setOrder, () => setOrder(null));
  }, [single]);

  const run = async (
    label: string,
    fn: (id: number) => Promise<unknown>,
    skip?: (t: TorrentListItem | undefined) => boolean,
  ) => {
    for (const id of ids) {
      if (skip?.(byId(id))) continue;
      try {
        await fn(id);
      } catch (e) {
        setCloseableError({
          text: `Error ${label} torrent id=${id}`,
          details: e as ErrorDetails,
        });
      }
    }
    refreshTorrents();
  };

  const fixErrors = () =>
    run("fixing errors on", async (id) => {
      const t = byId(id);
      const damaged = hasDamagedFiles(t?.stats?.damage);
      if (damaged && isRepairRunning(t?.stats?.damage)) return;
      const held = hasRecoveryIssues(t?.stats?.damage);
      if (!damaged && !held && t?.stats?.state !== STATE_ERROR) return;
      if (damaged && API.repairFiles) await API.repairFiles(id);
      else await API.fixErrors(id);
    });

  const move = async () => {
    const first = byId(ids[0]);
    const current = saveFolderOf(first?.output_folder ?? "", first?.name);
    const dest = window.prompt(
      `Move ${ids.length > 1 ? `${ids.length} torrents` : "torrent"} into folder (on the server). ` +
        "A multi-file torrent keeps its own folder: <folder>/<torrent name>.",
      current,
    );
    if (!dest || dest === current) return;
    await run("moving", (id) => API.relocateTorrent(id, dest, false, true));
  };

  const queue = async (action: QueueMoveAction) => {
    if (!API.queueMove) return;
    try {
      await API.queueMove(ids, action);
    } catch (e) {
      setCloseableError({
        text: "Error moving in queue",
        details: e as ErrorDetails,
      });
    }
    refreshTorrents();
  };

  const applyOrder = (patch: DownloadOrderPatch) =>
    run("setting download order on", async (id) => {
      await API.setDownloadOrder!(id, patch);
    });

  const anyError = selected.some(
    (t) =>
      t.stats?.state === STATE_ERROR ||
      hasDamagedFiles(t.stats?.damage) ||
      hasRecoveryIssues(t.stats?.damage),
  );
  const n = ids.length;
  const items: MenuItem[] = [
    {
      label: "Open details",
      disabled: single == null,
      onClick: () => single != null && openDetailsModal(single),
    },
    { separator: true },
    {
      label: n > 1 ? `Resume (${n})` : "Resume",
      disabled: states.every((s) => s === STATE_LIVE),
      onClick: () =>
        run("starting", (id) => API.start(id), (t) => t?.stats?.state === STATE_LIVE),
    },
    {
      label: n > 1 ? `Pause (${n})` : "Pause",
      disabled: states.every((s) => s === STATE_PAUSED),
      onClick: () =>
        run("pausing", (id) => API.pause(id), (t) => t?.stats?.state === STATE_PAUSED),
    },
    { label: "Restart", onClick: () => run("restarting", (id) => API.restart(id)) },
    { label: "Fix errors", disabled: !anyError, onClick: fixErrors },
    {
      label: "Force recheck",
      disabled: !API.recheck,
      onClick: () => run("rechecking", (id) => API.recheck!(id)),
    },
    { separator: true },
    {
      label: "Queue",
      disabled: !API.queueMove,
      submenu: [
        { label: "Move to top", onClick: () => queue("top") },
        { label: "Move up", onClick: () => queue("up") },
        { label: "Move down", onClick: () => queue("down") },
        { label: "Move to bottom", onClick: () => queue("bottom") },
      ],
    },
    {
      label: "Download order",
      disabled: !API.setDownloadOrder,
      submenu: torrentOrderMenu(order, applyOrder),
    },
    { label: "Move files…", onClick: move },
    { separator: true },
    {
      label: n > 1 ? `Remove ${n} torrents…` : "Remove…",
      danger: true,
      onClick: () =>
        setRemoving(ids.map((id) => ({ id, name: byId(id)?.name ?? null }))),
    },
  ];

  return (
    <>
      {menuOpen && (
        <ContextMenu
          x={x}
          y={y}
          items={items}
          testId="torrent-context-menu"
          onClose={() => {
            setMenuOpen(false);
            // Keep mounted while the remove dialog is open.
            setTimeout(() => {
              setRemoving((r) => {
                if (!r) onClose();
                return r;
              });
            }, 0);
          }}
        />
      )}
      {removing && (
        <DeleteTorrentModal
          show
          torrents={removing}
          onHide={() => {
            setRemoving(null);
            onClose();
          }}
        />
      )}
    </>
  );
};
