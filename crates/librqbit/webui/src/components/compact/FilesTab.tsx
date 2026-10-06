import { useContext, useEffect, useRef, useState } from "react";
import {
  TorrentDetails,
  TorrentStats,
  ErrorDetails,
  DownloadOrderPatch,
  DownloadOrderView,
} from "../../api-types";
import { APIContext } from "../../context";
import { FileListInput, FileUi } from "../FileListInput";
import { useErrorStore } from "../../stores/errorStore";
import { ContextMenu, MenuItem } from "../ContextMenu";
import { fileOrderMenu } from "../../helper/downloadOrderMenu";

interface FilesTabProps {
  torrentId: number;
  detailsResponse: TorrentDetails | null;
  statsResponse: TorrentStats | null;
  onRefresh?: () => void;
}

export const FilesTab: React.FC<FilesTabProps> = ({
  torrentId,
  detailsResponse,
  statsResponse,
  onRefresh,
}) => {
  const [selectedFiles, setSelectedFiles] = useState<Set<number>>(new Set());
  const [savingSelectedFiles, setSavingSelectedFiles] = useState(false);

  const API = useContext(APIContext);
  const setCloseableError = useErrorStore((state) => state.setCloseableError);

  // Highlighted files (click / ctrl+click / shift+click; separate from the
  // "included" checkboxes) and the right-click menu.
  const [highlighted, setHighlighted] = useState<Set<number>>(new Set());
  const anchor = useRef<number | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; ids: number[] } | null>(null);
  const [order, setOrder] = useState<DownloadOrderView | null>(null);

  const loadOrder = () => {
    if (!API.getDownloadOrder) return;
    API.getDownloadOrder(torrentId).then(setOrder, () => setOrder(null));
  };
  useEffect(() => {
    setHighlighted(new Set());
    anchor.current = null;
    setOrder(null);
    loadOrder();
  }, [torrentId]);

  const applyOrder = (patch: DownloadOrderPatch) => {
    API.setDownloadOrder?.(torrentId, patch).then(setOrder, (e) =>
      setCloseableError({
        text: "Error setting download order",
        details: e as ErrorDetails,
      }),
    );
  };

  const fileUi: FileUi = {
    highlighted,
    onHighlight: (id, e) => {
      if (e.shiftKey && anchor.current != null) {
        const [a, b] = [anchor.current, id].sort((x, y) => x - y);
        const next = new Set(e.ctrlKey || e.metaKey ? highlighted : []);
        for (let i = a; i <= b; i++) next.add(i);
        setHighlighted(next);
        return;
      }
      if (e.ctrlKey || e.metaKey) {
        const next = new Set(highlighted);
        next.has(id) ? next.delete(id) : next.add(id);
        setHighlighted(next);
      } else {
        setHighlighted(new Set([id]));
      }
      anchor.current = id;
    },
    onContextMenu: (ids, e) => {
      let target = ids;
      if (ids.length === 1) {
        if (highlighted.has(ids[0])) {
          target = Array.from(highlighted).sort((a, b) => a - b);
        } else {
          setHighlighted(new Set(ids));
          anchor.current = ids[0];
        }
      } else {
        setHighlighted(new Set(ids));
      }
      setMenu({ x: e.clientX, y: e.clientY, ids: target });
      loadOrder();
    },
    badge: (id) => {
      const f = order?.files.find((x) => x.id === id);
      if (!f) return undefined;
      const parts: string[] = [];
      if (f.override.sequential != null)
        parts.push(f.override.sequential ? "sequential" : "not sequential");
      if (f.override.first_last_first != null)
        parts.push(
          f.override.first_last_first ? "first+last first" : "no first+last first",
        );
      return parts.length ? `order: ${parts.join(", ")}` : undefined;
    },
  };

  useEffect(() => {
    setSelectedFiles(
      new Set<number>(
        detailsResponse?.files
          .map((f, id) => ({ f, id }))
          .filter(({ f }) => f.included)
          .map(({ id }) => id) ?? [],
      ),
    );
  }, [detailsResponse]);

  const updateSelectedFiles = (selectedFiles: Set<number>) => {
    setSavingSelectedFiles(true);
    API.updateOnlyFiles(torrentId, Array.from(selectedFiles))
      .then(
        () => {
          onRefresh?.();
          setCloseableError(null);
        },
        (e) => {
          setCloseableError({
            text: "Error configuring torrent",
            details: e as ErrorDetails,
          });
        },
      )
      .finally(() => setSavingSelectedFiles(false));
  };

  if (!detailsResponse) {
    return <div className="p-4 text-tertiary">Loading...</div>;
  }

  const renameSelected = async () => {
    const pick = highlighted.size === 1 ? highlighted : selectedFiles;
    if (pick.size !== 1) {
      setCloseableError({
        text: "Select exactly one file to rename",
        details: undefined,
      });
      return;
    }
    await renameFile(Array.from(pick)[0]);
  };

  const renameFile = async (fileId: number) => {
    const current = detailsResponse.files[fileId]?.name ?? "";
    const next = window.prompt("New relative path for file", current);
    if (!next || next === current) return;
    try {
      await API.renameFile(torrentId, fileId, next);
      onRefresh?.();
      setCloseableError(null);
    } catch (e) {
      setCloseableError({
        text: "Error renaming file",
        details: e as ErrorDetails,
      });
    }
  };

  const menuItems = (ids: number[]): MenuItem[] => {
    const n = ids.length;
    const allIncluded = ids.every((id) => selectedFiles.has(id));
    return [
      {
        label: n > 1 ? `${n} files` : detailsResponse.files[ids[0]]?.name ?? "File",
        disabled: true,
      },
      { separator: true },
      ...(API.setDownloadOrder ? fileOrderMenu(order, ids, applyOrder) : []),
      {
        label: "Reset download order for these files",
        disabled: !API.setDownloadOrder,
        onClick: () =>
          applyOrder({
            files: [{ ids, sequential: null, first_last_first: null }],
          }),
      },
      { separator: true },
      {
        label: allIncluded ? "Don't download" : "Download",
        onClick: () => {
          const next = new Set(selectedFiles);
          ids.forEach((id) => (allIncluded ? next.delete(id) : next.add(id)));
          updateSelectedFiles(next);
        },
      },
      {
        label: "Rename…",
        disabled: n !== 1,
        onClick: () => renameFile(ids[0]),
      },
    ];
  };

  return (
    <div className="p-2 text-sm">
      {order && (
        <div className="mb-2 text-xs text-tertiary" data-testid="download-order-summary">
          Download order: {order.summary}. Right-click files (or a folder;
          ctrl/shift-click to pick several) to change per-file order.
        </div>
      )}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems(menu.ids)}
          testId="file-context-menu"
          onClose={() => setMenu(null)}
        />
      )}
      <div className="mb-2 flex gap-2">
        <button
          type="button"
          className="px-2 py-1 rounded border border-divider text-xs hover:bg-surface-raised"
          onClick={renameSelected}
          disabled={savingSelectedFiles}
        >
          Rename selected
        </button>
      </div>
      <FileListInput
        torrentId={torrentId}
        torrentDetails={detailsResponse}
        torrentStats={statsResponse}
        selectedFiles={selectedFiles}
        setSelectedFiles={updateSelectedFiles}
        disabled={savingSelectedFiles}
        allowStream
        showProgressBar
        fileUi={fileUi}
      />
    </div>
  );
};
