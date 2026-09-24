import { useContext, useMemo, useRef, useState } from "react";
import { APIContext } from "../../context";
import { useTorrentStore } from "../../stores/torrentStore";
import { Modal } from "./Modal";
import { ModalBody } from "./ModalBody";
import { ModalFooter } from "./ModalFooter";
import { Button } from "../buttons/Button";
import { FormInput } from "../forms/FormInput";
import { FormCheckbox } from "../forms/FormCheckbox";
import { ProgressBar } from "../ProgressBar";
import { extractMagnetLinks } from "../../helper/parseMagnets";
import {
  BulkImportProgress,
  BulkWorkItem,
  runBulkQueue,
} from "../../helper/bulkImportQueue";

const DEFAULT_CONCURRENCY = 4;

type Tab = "magnets" | "files";

type Props = {
  isOpen: boolean;
  onClose: () => void;
  /** Optional pre-selected .torrent files (e.g. from multi FileInput). */
  initialFiles?: File[];
  /** Optional paste text to prefill the magnets tab. */
  initialPaste?: string;
};

export const BulkImportModal: React.FC<Props> = ({
  isOpen,
  onClose,
  initialFiles,
  initialPaste,
}) => {
  const API = useContext(APIContext);
  const refreshTorrents = useTorrentStore((s) => s.refreshTorrents);

  const [tab, setTab] = useState<Tab>(
    initialFiles && initialFiles.length > 0 ? "files" : "magnets",
  );
  const [pasteText, setPasteText] = useState(initialPaste ?? "");
  const [files, setFiles] = useState<File[]>(initialFiles ?? []);
  const [outputFolder, setOutputFolder] = useState("");
  const [overwrite, setOverwrite] = useState(true);
  const [concurrency, setConcurrency] = useState(DEFAULT_CONCURRENCY);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<BulkImportProgress | null>(null);
  const cancelRef = useRef({ cancelled: false });
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [dragOver, setDragOver] = useState(false);

  const magnets = useMemo(() => extractMagnetLinks(pasteText), [pasteText]);

  const itemCount = tab === "magnets" ? magnets.length : files.length;
  const canStart = !running && itemCount > 0;

  const resetForm = () => {
    setPasteText("");
    setFiles([]);
    setOutputFolder("");
    setOverwrite(true);
    setConcurrency(DEFAULT_CONCURRENCY);
    setProgress(null);
    setRunning(false);
    cancelRef.current = { cancelled: false };
    setTab("magnets");
    setDragOver(false);
  };

  const handleClose = () => {
    if (running) {
      cancelRef.current.cancelled = true;
    }
    resetForm();
    onClose();
  };

  const onFilesPicked = (list: FileList | File[] | null) => {
    if (!list) return;
    const next = Array.from(list).filter((f) =>
      f.name.toLowerCase().endsWith(".torrent"),
    );
    setFiles((prev) => {
      const seen = new Set(prev.map((f) => `${f.name}:${f.size}:${f.lastModified}`));
      const merged = [...prev];
      for (const f of next) {
        const key = `${f.name}:${f.size}:${f.lastModified}`;
        if (!seen.has(key)) {
          seen.add(key);
          merged.push(f);
        }
      }
      return merged;
    });
    setTab("files");
  };

  const startImport = async () => {
    cancelRef.current = { cancelled: false };
    setRunning(true);
    setProgress(null);

    const opts = {
      overwrite,
      output_folder: outputFolder.trim() || undefined,
    };

    let work: BulkWorkItem<string | File>[];
    if (tab === "magnets") {
      work = magnets.map((m, i) => ({
        id: `m-${i}`,
        label: m.length > 72 ? m.slice(0, 69) + "…" : m,
        data: m,
      }));
    } else {
      work = files.map((f, i) => ({
        id: `f-${i}`,
        label: f.name,
        data: f,
      }));
    }

    try {
      const result = await runBulkQueue({
        items: work,
        concurrency,
        signal: cancelRef.current,
        onProgress: setProgress,
        worker: async (data) => {
          await API.uploadTorrent(data, opts);
        },
      });
      setProgress(result);
      if (result.ok > 0) {
        refreshTorrents();
      }
    } finally {
      setRunning(false);
    }
  };

  const stopQueue = () => {
    cancelRef.current.cancelled = true;
  };

  const pct =
    progress && progress.total > 0
      ? (100 * progress.done) / progress.total
      : 0;

  const failedItems =
    progress?.items.filter((i) => i.status === "error").slice(0, 50) ?? [];

  return (
    <Modal
      isOpen={isOpen}
      onClose={running ? undefined : handleClose}
      title="Import many"
    >
      <ModalBody>
        <div className="flex gap-2 mb-3">
          <Button
            variant={tab === "magnets" ? "primary" : "secondary"}
            size="sm"
            onClick={() => !running && setTab("magnets")}
            disabled={running}
          >
            Paste magnets
          </Button>
          <Button
            variant={tab === "files" ? "primary" : "secondary"}
            size="sm"
            onClick={() => !running && setTab("files")}
            disabled={running}
          >
            .torrent files
          </Button>
        </div>

        {tab === "magnets" ? (
          <div className="flex flex-col gap-2 mb-3">
            <label htmlFor="bulk_magnets">
              Paste magnet links (one per line or whitespace-separated)
            </label>
            <textarea
              id="bulk_magnets"
              className="block w-full min-h-40 border border-divider rounded bg-transparent py-1.5 px-2 font-mono text-sm focus:ring-0 focus:border-primary"
              placeholder={"magnet:?xt=urn:btih:...\nmagnet:?xt=urn:btih:..."}
              value={pasteText}
              disabled={running}
              onChange={(e) => setPasteText(e.target.value)}
            />
            <div className="text-sm text-secondary">
              {magnets.length === 0
                ? "No magnets detected yet."
                : `${magnets.length} magnet${magnets.length === 1 ? "" : "s"} detected.`}
            </div>
          </div>
        ) : (
          <div className="flex flex-col gap-2 mb-3">
            <input
              ref={fileInputRef}
              type="file"
              accept=".torrent"
              multiple
              hidden
              disabled={running}
              onChange={(e) => {
                onFilesPicked(e.target.files);
                e.target.value = "";
              }}
            />
            <div
              className={`border-2 border-dashed rounded p-6 text-center transition-colors ${
                dragOver
                  ? "border-primary bg-primary/10"
                  : "border-divider bg-surface"
              } ${running ? "opacity-50 pointer-events-none" : "cursor-pointer"}`}
              onClick={() => fileInputRef.current?.click()}
              onDragOver={(e) => {
                e.preventDefault();
                setDragOver(true);
              }}
              onDragLeave={() => setDragOver(false)}
              onDrop={(e) => {
                e.preventDefault();
                setDragOver(false);
                onFilesPicked(e.dataTransfer.files);
              }}
            >
              <div className="font-medium">
                Drop .torrent files here, or click to select
              </div>
              <div className="text-sm text-secondary mt-1">
                Multiple files supported (thousands OK — queued).
              </div>
            </div>
            {files.length > 0 && (
              <div className="text-sm flex items-center justify-between gap-2">
                <span>
                  {files.length} file{files.length === 1 ? "" : "s"} selected
                </span>
                <Button
                  size="sm"
                  variant="cancel"
                  disabled={running}
                  onClick={() => setFiles([])}
                >
                  Clear files
                </Button>
              </div>
            )}
          </div>
        )}

        <FormInput
          label="Output folder (optional)"
          name="bulk_output_folder"
          value={outputFolder}
          disabled={running}
          onChange={(e) => setOutputFolder(e.target.value)}
          placeholder="Leave empty for session default"
        />

        <div className="mb-3">
          <FormCheckbox
            name="bulk_overwrite"
            label="Overwrite existing files on disk"
            checked={overwrite}
            disabled={running}
            onChange={() => setOverwrite(!overwrite)}
          />
        </div>

        <div className="flex flex-col gap-1 mb-3">
          <label htmlFor="bulk_concurrency">
            Concurrent adds: {concurrency}
          </label>
          <input
            id="bulk_concurrency"
            type="range"
            min={2}
            max={8}
            step={1}
            value={concurrency}
            disabled={running}
            onChange={(e) => setConcurrency(Number(e.target.value))}
            className="w-full"
          />
          <div className="text-sm text-secondary">
            Keeps the server from being flooded when importing thousands.
          </div>
        </div>

        {progress && (
          <div className="mt-2 mb-2">
            <ProgressBar
              now={pct}
              label={`${progress.done}/${progress.total} · ${progress.ok} ok · ${progress.failed} failed${
                progress.cancelled ? ` · ${progress.cancelled} cancelled` : ""
              }`}
              variant={
                progress.failed > 0
                  ? "warn"
                  : progress.done >= progress.total
                    ? "success"
                    : "info"
              }
            />
            {failedItems.length > 0 && (
              <div className="mt-2 max-h-40 overflow-y-auto text-sm border border-divider rounded p-2">
                <div className="font-medium mb-1">
                  Failures
                  {progress.failed > failedItems.length
                    ? ` (showing ${failedItems.length} of ${progress.failed})`
                    : ""}
                  :
                </div>
                <ul className="list-disc pl-5 space-y-1">
                  {failedItems.map((item) => (
                    <li key={item.id}>
                      <span className="font-mono break-all">{item.label}</span>
                      {item.error ? (
                        <span className="text-secondary"> — {item.error}</span>
                      ) : null}
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        )}
      </ModalBody>

      <ModalFooter>
        {running ? (
          <Button variant="danger" onClick={stopQueue}>
            Stop queue
          </Button>
        ) : (
          <Button variant="cancel" onClick={handleClose}>
            {progress ? "Close" : "Cancel"}
          </Button>
        )}
        <Button
          variant="primary"
          disabled={!canStart}
          onClick={startImport}
        >
          {running
            ? "Importing…"
            : `Import ${itemCount || ""}`.trim()}
        </Button>
      </ModalFooter>
    </Modal>
  );
};

