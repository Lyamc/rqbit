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
import { TabButton, TabList } from "../Tabs";
import { FilesystemBrowser } from "../filesystem/FilesystemBrowser";
import { StagingItem, StagingQueue } from "../add/StagingQueue";
import { UrlLinesEditor } from "../add/UrlLinesEditor";
import { extractTorrentSources } from "../../helper/parseTorrentSources";
import {
  TransferCandidate,
  displayNameForSource,
  matchTransferFolders,
} from "../../helper/matchTransferFolders";
import {
  BulkImportProgress,
  BulkWorkItem,
  runBulkQueue,
} from "../../helper/bulkImportQueue";

const DEFAULT_CONCURRENCY = 4;

type Tab = "upload" | "urls" | "browse";

type Props = {
  isOpen: boolean;
  onClose: () => void;
  initialFiles?: File[];
  initialPaste?: string;
  initialTab?: Tab;
};

let idSeq = 0;
const nextId = () => `stg-${Date.now()}-${++idSeq}`;

function fileKey(f: File) {
  return `${f.name}:${f.size}:${f.lastModified}`;
}

export const AddModal: React.FC<Props> = ({
  isOpen,
  onClose,
  initialFiles,
  initialPaste,
  initialTab,
}) => {
  const API = useContext(APIContext);
  const refreshTorrents = useTorrentStore((s) => s.refreshTorrents);

  const [tab, setTab] = useState<Tab>(
    initialTab ?? (initialFiles && initialFiles.length > 0 ? "upload" : "urls"),
  );
  const [pasteText, setPasteText] = useState(initialPaste ?? "");
  const [queue, setQueue] = useState<StagingItem[]>(() => {
    const items: StagingItem[] = [];
    if (initialFiles) {
      for (const f of initialFiles) {
        if (f.name.toLowerCase().endsWith(".torrent")) {
          items.push({
            id: nextId(),
            label: f.name,
            source: "upload",
            status: "ready",
            kind: "file",
            file: f,
          });
        }
      }
    }
    if (initialPaste) {
      for (const s of extractTorrentSources(initialPaste)) {
        items.push({
          id: nextId(),
          label: displayNameForSource(s.value),
          source: "url",
          status: "ready",
          kind: s.kind,
          text: s.value,
        });
      }
    }
    return items;
  });
  const [outputFolder, setOutputFolder] = useState("");
  const [overwrite, setOverwrite] = useState(true);
  const [transferMode, setTransferMode] = useState(false);
  const [transferBusy, setTransferBusy] = useState(false);
  const [transferMsg, setTransferMsg] = useState<string | null>(null);
  const [concurrency, setConcurrency] = useState(DEFAULT_CONCURRENCY);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<BulkImportProgress | null>(null);
  const [uploadBusy, setUploadBusy] = useState(false);
  const [uploadMsg, setUploadMsg] = useState<string | null>(null);
  const cancelRef = useRef({ cancelled: false });
  const fileInputRef = useRef<HTMLInputElement>(null);
  const folderInputRef = useRef<HTMLInputElement>(null);
  const [dragOver, setDragOver] = useState(false);

  const detectedUrls = useMemo(
    () => extractTorrentSources(pasteText),
    [pasteText],
  );

  const readyCount = queue.filter(
    (i) => i.status === "ready" || i.status === "pending",
  ).length;
  const canStart = !running && readyCount > 0;

  const mergeIntoQueue = (incoming: StagingItem[]) => {
    setQueue((prev) => {
      const seen = new Set(
        prev.map((p) => {
          if (p.kind === "file" && p.file) return `file:${fileKey(p.file)}`;
          if (p.text) return `text:${p.text}`;
          if (p.serverPath) return `srv:${p.serverPath}`;
          if (p.bytes) return `bytes:${p.label}:${p.bytes.byteLength}`;
          return p.id;
        }),
      );
      const merged = [...prev];
      for (const item of incoming) {
        let key: string;
        if (item.kind === "file" && item.file)
          key = `file:${fileKey(item.file)}`;
        else if (item.text) key = `text:${item.text}`;
        else if (item.serverPath) key = `srv:${item.serverPath}`;
        else if (item.bytes)
          key = `bytes:${item.label}:${item.bytes.byteLength}`;
        else key = item.id;
        if (seen.has(key)) continue;
        seen.add(key);
        merged.push(item);
      }
      return merged;
    });
  };

  const resetForm = () => {
    setPasteText("");
    setQueue([]);
    setOutputFolder("");
    setOverwrite(true);
    setTransferMode(false);
    setTransferBusy(false);
    setTransferMsg(null);
    setConcurrency(DEFAULT_CONCURRENCY);
    setProgress(null);
    setRunning(false);
    cancelRef.current = { cancelled: false };
    setTab("urls");
    setDragOver(false);
    setUploadBusy(false);
    setUploadMsg(null);
  };

  const handleClose = () => {
    if (running) {
      cancelRef.current.cancelled = true;
    }
    resetForm();
    onClose();
  };

  const enqueueTorrentFiles = (list: FileList | File[] | null) => {
    if (!list) return;
    const files = Array.from(list);
    const torrents: StagingItem[] = [];
    const zips: File[] = [];
    const other: string[] = [];

    for (const f of files) {
      const lower = f.name.toLowerCase();
      if (lower.endsWith(".torrent")) {
        torrents.push({
          id: nextId(),
          label: f.webkitRelativePath || f.name,
          source: "upload",
          status: "ready",
          kind: "file",
          file: f,
        });
      } else if (lower.endsWith(".zip")) {
        zips.push(f);
      } else if (
        lower.endsWith(".txt") ||
        lower.endsWith(".magnet") ||
        lower.endsWith(".md")
      ) {
        // client-side magnet scan
        other.push(f.name);
        void f.text().then((text) => {
          const sources = extractTorrentSources(text);
          mergeIntoQueue(
            sources.map((s) => ({
              id: nextId(),
              label: s.value.length > 64 ? s.value.slice(0, 61) + "…" : s.value,
              source: `upload:${f.name}`,
              status: "ready" as const,
              kind: s.kind,
              text: s.value,
            })),
          );
        });
      } else {
        other.push(f.name);
        mergeIntoQueue([
          {
            id: nextId(),
            label: f.name,
            source: "upload",
            status: "error",
            kind: "file",
            error: "not a .torrent, .zip, or magnet text file",
          },
        ]);
      }
    }

    if (torrents.length) mergeIntoQueue(torrents);
    for (const z of zips) {
      void extractZipOnServer(z);
    }
  };

  const extractZipOnServer = async (file: File) => {
    setUploadBusy(true);
    setUploadMsg(`Extracting ${file.name}…`);
    try {
      const res = await API.extractUpload(file);
      const staged: StagingItem[] = [];
      for (const it of res.items) {
        if (it.kind === "torrent" && it.data_base64) {
          const bin = Uint8Array.from(atob(it.data_base64), (c) =>
            c.charCodeAt(0),
          );
          staged.push({
            id: nextId(),
            label: it.name,
            source: `zip:${file.name}`,
            status: "ready",
            kind: "torrent_bytes",
            bytes: bin,
          });
        } else if (it.kind === "magnet" && it.magnet) {
          staged.push({
            id: nextId(),
            label:
              it.magnet.length > 64 ? it.magnet.slice(0, 61) + "…" : it.magnet,
            source: `zip:${file.name}`,
            status: "ready",
            kind: "magnet",
            text: it.magnet,
          });
        } else if (it.kind === "error" || it.kind === "skipped") {
          // Only surface errors (not every skipped junk file) when explicitly error
          if (it.kind === "error") {
            staged.push({
              id: nextId(),
              label: it.name,
              source: `zip:${file.name}`,
              status: "error",
              kind: "file",
              error: it.error || "extract error",
            });
          }
        }
      }
      mergeIntoQueue(staged);
      setUploadMsg(
        `From ${file.name}: ${staged.filter((s) => s.status === "ready").length} ready` +
          (staged.some((s) => s.status === "error")
            ? `, ${staged.filter((s) => s.status === "error").length} errors`
            : ""),
      );
    } catch (e: unknown) {
      const err = e as { text?: string; message?: string };
      mergeIntoQueue([
        {
          id: nextId(),
          label: file.name,
          source: "zip",
          status: "error",
          kind: "file",
          error: err?.text || err?.message || String(e),
        },
      ]);
      setUploadMsg(null);
    } finally {
      setUploadBusy(false);
    }
  };

  const addDetectedUrls = () => {
    mergeIntoQueue(
      detectedUrls.map((s) => ({
        id: nextId(),
        label: displayNameForSource(s.value),
        source: "url",
        status: "ready" as const,
        kind: s.kind,
        text: s.value,
      })),
    );
    setPasteText("");
  };

  const onBrowseConfirm = (paths: string[]) => {
    mergeIntoQueue(
      paths.map((p) => {
        const name = p.split(/[/\\]/).pop() || p;
        return {
          id: nextId(),
          label: name,
          source: "browse",
          status: "ready" as const,
          kind: "server_path" as const,
          serverPath: p,
        };
      }),
    );
  };

  const clearTransferMatches = () => {
    setQueue((prev) =>
      prev.map((i) => ({
        ...i,
        matchedPath: undefined,
        matchStatus: undefined,
        matchConfidence: undefined,
        matchReason: undefined,
      })),
    );
    setTransferMsg(null);
  };

  const onTransferFolders = async (selectedPaths: string[]) => {
    if (!selectedPaths.length) return;
    setTransferBusy(true);
    setTransferMsg("Scanning folders…");
    try {
      const candidates: TransferCandidate[] = [];
      const seen = new Set<string>();
      const pushCand = (path: string, name: string, children?: string[]) => {
        if (seen.has(path)) return;
        seen.add(path);
        candidates.push({ path, name, children });
      };

      const discoveredTorrents: StagingItem[] = [];

      for (const p of selectedPaths) {
        const name = p.split(/[/\\]/).pop() || p;
        try {
          const listing = await API.fsList(p);
          const children = listing.entries.map((e) => e.name);
          pushCand(p, name, children);
          for (const e of listing.entries) {
            if (!e.is_dir) continue;
            try {
              const sub = await API.fsList(e.path);
              pushCand(
                e.path,
                e.name,
                sub.entries.map((x) => x.name),
              );
            } catch {
              pushCand(e.path, e.name);
            }
          }
          for (const e of listing.entries) {
            if (!e.is_torrent) continue;
            discoveredTorrents.push({
              id: nextId(),
              label: e.name.replace(/\.torrent$/i, ""),
              source: "transfer",
              status: "ready",
              kind: "server_path",
              serverPath: e.path,
            });
          }
        } catch (err: unknown) {
          const e = err as { text?: string; message?: string };
          pushCand(p, name);
          setTransferMsg(e?.text || e?.message || String(err));
        }
      }

      setQueue((prev) => {
        // Merge newly discovered .torrent paths (dedupe by serverPath/text).
        const seenKeys = new Set(
          prev.map((p) => {
            if (p.serverPath) return `srv:${p.serverPath}`;
            if (p.text) return `text:${p.text}`;
            return p.id;
          }),
        );
        const merged = [...prev];
        for (const item of discoveredTorrents) {
          const key = item.serverPath ? `srv:${item.serverPath}` : item.id;
          if (seenKeys.has(key)) continue;
          seenKeys.add(key);
          merged.push(item);
        }

        const hints = merged.map((i) => ({ id: i.id, label: i.label }));
        const matches = matchTransferFolders(hints, candidates);
        const byId = new Map(matches.map((m) => [m.torrentId, m]));
        let matched = 0;
        let ambiguous = 0;
        let unmatched = 0;
        const next = merged.map((item) => {
          const m = byId.get(item.id);
          if (!m) return item;
          if (m.status === "matched") matched++;
          else if (m.status === "ambiguous") ambiguous++;
          else unmatched++;
          return {
            ...item,
            matchedPath: m.path,
            matchStatus: m.status,
            matchConfidence: m.confidence,
            matchReason: m.reason,
          };
        });
        setTransferMsg(
          `Transfer: ${candidates.length} folder${candidates.length === 1 ? "" : "s"} · ` +
            `${matched} matched · ${ambiguous} ambiguous · ${unmatched} unmatched` +
            (discoveredTorrents.length
              ? ` · ${discoveredTorrents.length} .torrent found`
              : ""),
        );
        return next;
      });
    } finally {
      setTransferBusy(false);
    }
  };

  const startImport = async () => {
    cancelRef.current = { cancelled: false };
    setRunning(true);
    setProgress(null);

    const workItems = queue.filter(
      (i) => i.status === "ready" || i.status === "pending",
    );

    // Mark them pending in UI
    setQueue((prev) =>
      prev.map((i) =>
        workItems.some((w) => w.id === i.id)
          ? { ...i, status: "pending", error: undefined }
          : i,
      ),
    );

    const work: BulkWorkItem<StagingItem>[] = workItems.map((i) => ({
      id: i.id,
      label: i.label,
      data: i,
    }));

    try {
      const result = await runBulkQueue({
        items: work,
        concurrency,
        signal: cancelRef.current,
        onProgress: (p) => {
          setProgress(p);
          setQueue((prev) => {
            const byId = new Map(p.items.map((x) => [x.id, x]));
            return prev.map((item) => {
              const st = byId.get(item.id);
              if (!st) return item;
              return {
                ...item,
                status: st.status as StagingItem["status"],
                error: st.error,
              };
            });
          });
        },
        worker: async (item) => {
          const itemOpts = {
            overwrite: transferMode ? true : overwrite,
            output_folder: item.matchedPath || outputFolder.trim() || undefined,
          };
          if (item.kind === "server_path" && item.serverPath) {
            await API.uploadTorrentFromServerPath(item.serverPath, itemOpts);
          } else if (item.kind === "torrent_bytes" && item.bytes) {
            const name = item.label.endsWith(".torrent")
              ? item.label
              : `${item.label}.torrent`;
            const file = new File(
              [
                item.bytes.buffer.slice(
                  item.bytes.byteOffset,
                  item.bytes.byteOffset + item.bytes.byteLength,
                ) as ArrayBuffer,
              ],
              name,
              { type: "application/x-bittorrent" },
            );
            await API.uploadTorrent(file, itemOpts);
          } else if (item.file) {
            await API.uploadTorrent(item.file, itemOpts);
          } else if (item.text) {
            await API.uploadTorrent(item.text, itemOpts);
          } else {
            throw new Error("nothing to upload");
          }
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
    progress && progress.total > 0 ? (100 * progress.done) / progress.total : 0;

  return (
    <Modal
      isOpen={isOpen}
      onClose={running ? undefined : handleClose}
      title="Add torrents"
      className="sm:max-w-3xl"
    >
      <ModalBody>
        <TabList className="mb-3">
          <TabButton
            id="upload"
            label="Upload"
            active={tab === "upload"}
            onClick={() => !running && setTab("upload")}
          />
          <TabButton
            id="urls"
            label="Import from URL(s)"
            active={tab === "urls"}
            onClick={() => !running && setTab("urls")}
          />
          <TabButton
            id="browse"
            label="Browse server"
            active={tab === "browse"}
            onClick={() => !running && setTab("browse")}
          />
        </TabList>

        <div className="mb-3 flex flex-col gap-2">
          <FormCheckbox
            name="add_transfer"
            label="Transfer from other client"
            checked={transferMode}
            disabled={running || transferBusy}
            onChange={() => {
              const next = !transferMode;
              setTransferMode(next);
              if (!next) clearTransferMatches();
              else setOverwrite(true);
            }}
          />
          {transferMode && (
            <div className="border border-divider rounded p-2">
              <div className="text-sm text-secondary mb-2">
                Select folders that already contain downloads (or a parent of
                many). We&apos;ll fuzzy-match them to the queue and set each
                item&apos;s output folder.
              </div>
              <FilesystemBrowser
                mode="select-directories"
                multi
                confirmLabel="Match folders"
                onConfirm={(paths) => {
                  void onTransferFolders(paths);
                }}
              />
              {transferMsg && (
                <div className="text-sm text-secondary mt-2">{transferMsg}</div>
              )}
              {transferBusy && (
                <div className="text-sm text-secondary mt-1">Working…</div>
              )}
            </div>
          )}
        </div>

        {tab === "upload" && (
          <div className="flex flex-col gap-2 mb-3">
            <input
              ref={fileInputRef}
              type="file"
              accept=".torrent,.zip,.txt,.magnet"
              multiple
              hidden
              disabled={running || uploadBusy}
              onChange={(e) => {
                enqueueTorrentFiles(e.target.files);
                e.target.value = "";
              }}
            />
            <input
              ref={folderInputRef}
              type="file"
              // @ts-expect-error webkitdirectory is non-standard but widely supported
              webkitdirectory=""
              multiple
              hidden
              disabled={running || uploadBusy}
              onChange={(e) => {
                enqueueTorrentFiles(e.target.files);
                e.target.value = "";
              }}
            />
            <div
              className={`border-2 border-dashed rounded p-6 text-center transition-colors ${
                dragOver
                  ? "border-primary bg-primary/10"
                  : "border-divider bg-surface"
              } ${running || uploadBusy ? "opacity-50 pointer-events-none" : "cursor-pointer"}`}
              onClick={() => fileInputRef.current?.click()}
              onDragOver={(e) => {
                e.preventDefault();
                setDragOver(true);
              }}
              onDragLeave={() => setDragOver(false)}
              onDrop={(e) => {
                e.preventDefault();
                setDragOver(false);
                enqueueTorrentFiles(e.dataTransfer.files);
              }}
            >
              <div className="font-medium">
                Drop files, folders, or zips here — or click to select
              </div>
              <div className="text-sm text-secondary mt-1">
                .torrent files are staged locally. Zips are unpacked
                server-side. Non-torrent junk is skipped with an error in the
                queue.
              </div>
            </div>
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="secondary"
                disabled={running || uploadBusy}
                onClick={() => fileInputRef.current?.click()}
              >
                Add files
              </Button>
              <Button
                size="sm"
                variant="secondary"
                disabled={running || uploadBusy}
                onClick={() => folderInputRef.current?.click()}
              >
                Add folder
              </Button>
            </div>
            {uploadMsg && (
              <div className="text-sm text-secondary">{uploadMsg}</div>
            )}
          </div>
        )}

        {tab === "urls" && (
          <div className="flex flex-col gap-2 mb-3">
            <UrlLinesEditor
              id="add_urls"
              value={pasteText}
              disabled={running}
              placeholder={"magnet:?xt=urn:btih:…"}
              onChange={setPasteText}
            />
            <div className="flex items-center justify-between gap-2">
              <div className="text-sm text-secondary">
                {pasteText.trim().length === 0
                  ? "Paste one URL, or several (one per line)."
                  : detectedUrls.length === 0
                    ? "No magnets or torrent URLs detected yet."
                    : `${detectedUrls.length} source${detectedUrls.length === 1 ? "" : "s"} detected.`}
              </div>
              <Button
                size="sm"
                variant="primary"
                disabled={running || detectedUrls.length === 0}
                onClick={addDetectedUrls}
              >
                Add to queue
              </Button>
            </div>
          </div>
        )}

        {tab === "browse" && (
          <div className="mb-3">
            <FilesystemBrowser
              mode="select-torrents"
              multi
              onConfirm={onBrowseConfirm}
              confirmLabel="Add to queue"
            />
          </div>
        )}

        {queue.length > 0 && (
          <>
            <StagingQueue
              items={queue}
              running={running}
              onRemove={(id) =>
                setQueue((prev) => prev.filter((i) => i.id !== id))
              }
              onClear={() => setQueue([])}
              onDismissErrors={() =>
                setQueue((prev) => prev.filter((i) => i.status !== "error"))
              }
            />

            <div className="mt-3">
              <FormInput
                label="Output folder (optional)"
                name="add_output_folder"
                value={outputFolder}
                disabled={running}
                onChange={(e) => setOutputFolder(e.target.value)}
                placeholder="Leave empty for session default"
              />
            </div>

            <div className="mb-3">
              <FormCheckbox
                name="add_overwrite"
                label="Overwrite existing files on disk"
                checked={overwrite}
                disabled={running}
                onChange={() => setOverwrite(!overwrite)}
              />
            </div>

            <div className="flex flex-col gap-1 mb-3">
              <label htmlFor="add_concurrency">
                Concurrent adds: {concurrency}
              </label>
              <input
                id="add_concurrency"
                type="range"
                min={2}
                max={8}
                step={1}
                value={concurrency}
                disabled={running}
                onChange={(e) => setConcurrency(Number(e.target.value))}
                className="w-full"
              />
            </div>
          </>
        )}

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
        <Button variant="primary" disabled={!canStart} onClick={startImport}>
          {running ? "Adding…" : `Add ${readyCount || ""}`.trim()}
        </Button>
      </ModalFooter>
    </Modal>
  );
};

/** Back-compat alias while old imports migrate. */
export { AddModal as BulkImportModal };
