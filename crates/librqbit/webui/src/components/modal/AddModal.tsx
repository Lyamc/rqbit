import { useContext, useEffect, useMemo, useRef, useState } from "react";
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
  TransferCandidateChild,
  displayNameForSource,
  matchTransferFolders,
  stripPartialSuffix,
} from "../../helper/matchTransferFolders";
import { AddTorrentResponse, FsEntry } from "../../api-types";
import {
  BulkImportProgress,
  BulkWorkItem,
  runBulkQueue,
} from "../../helper/bulkImportQueue";

const DEFAULT_CONCURRENCY = 4;

// POST /torrents for a magnet doesn't return until the torrent's metadata has
// been fetched from peers (DHT/trackers), and the server applies no timeout.
// Give up after this long so a dead magnet shows an error instead of hanging.
// Aborting the request closes the connection, which also cancels the
// server-side resolve.
const MAGNET_TIMEOUT_MS = 3 * 60_000;
const OTHER_TIMEOUT_MS = 2 * 60_000;

const STOPPED = "Stopped";

// Transfer-from-other-client folder scan limits (selected folder = depth 0).
const MAX_TRANSFER_SCAN_DEPTH = 3;
const MAX_TRANSFER_SCAN_DIRS = 3000;

const ADVANCED_OPEN_KEY = "rqbit.addModal.advancedOpen";
const readAdvancedOpen = () => {
  try {
    return window.sessionStorage.getItem(ADVANCED_OPEN_KEY) === "1";
  } catch {
    return false;
  }
};

const isMagnetItem = (i: StagingItem) =>
  i.kind === "magnet" ||
  (!!i.text &&
    (i.text.startsWith("magnet:") || /^[0-9a-fA-F]{40}$/.test(i.text)));

/** Run `fn` over `items` with at most `limit` in flight. */
async function mapLimit<T>(
  items: T[],
  limit: number,
  fn: (item: T) => Promise<void>,
): Promise<void> {
  let next = 0;
  const workers = Array.from(
    { length: Math.min(limit, items.length) },
    async () => {
      while (next < items.length) {
        const i = next++;
        await fn(items[i]);
      }
    },
  );
  await Promise.all(workers);
}

const errText = (e: unknown) => {
  const err = e as { text?: string; message?: string };
  return String(err?.text || err?.message || e);
};

type ItemOpts = {
  overwrite: boolean;
  output_folder?: string;
  adopt_foreign_incomplete?: "qbit";
};

const formatDuration = (ms: number) => {
  const m = Math.round(ms / 60_000);
  return m >= 1 ? `${m} min` : `${Math.round(ms / 1000)} s`;
};

export type AddModalTab = "upload" | "urls" | "browse";
type Tab = AddModalTab;

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
  const queueRef = useRef(queue);
  queueRef.current = queue;
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
  // In-flight add requests, so Stop / close can abort them.
  const abortersRef = useRef(new Map<string, AbortController>());
  const [advancedOpen, setAdvancedOpen] = useState(readAdvancedOpen);
  useEffect(() => {
    try {
      window.sessionStorage.setItem(
        ADVANCED_OPEN_KEY,
        advancedOpen ? "1" : "0",
      );
    } catch {
      // ignore (private mode etc.)
    }
  }, [advancedOpen]);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const folderInputRef = useRef<HTMLInputElement>(null);
  const [dragOver, setDragOver] = useState(false);

  const detectedUrls = useMemo(
    () => extractTorrentSources(pasteText),
    [pasteText],
  );

  // Sources typed/pasted in the URL field but not staged yet: the footer Add
  // button stages and starts them in one click.
  const pastedNotQueued = useMemo(() => {
    const queued = new Set(queue.map((q) => q.text).filter(Boolean));
    return detectedUrls.filter((s) => !queued.has(s.value));
  }, [detectedUrls, queue]);

  // In transfer mode only items matched to existing data are added; the rest
  // stay staged for review (turn transfer off to add them as fresh downloads).
  const isStartable = (i: StagingItem) =>
    (i.status === "ready" || i.status === "pending") &&
    (!transferMode || i.matchStatus === "matched");
  const readyCount =
    queue.filter(isStartable).length +
    (transferMode ? 0 : pastedNotQueued.length);
  const canStart = !running && readyCount > 0;
  const inFlight = queue.filter(
    (i) => i.status === "running" || i.status === "resolving",
  );
  const advancedSummary = [
    transferMode ? "transfer from other client" : null,
    outputFolder.trim() ? "custom output folder" : null,
    !overwrite ? "no overwrite" : null,
    concurrency !== DEFAULT_CONCURRENCY ? `${concurrency} at once` : null,
  ].filter((x): x is string => !!x);
  const resolvingCount = inFlight.filter(
    (i) => i.status === "resolving",
  ).length;

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

  const abortAll = () => {
    for (const c of abortersRef.current.values()) c.abort();
    abortersRef.current.clear();
  };

  const handleClose = () => {
    if (running) {
      cancelRef.current.cancelled = true;
      abortAll();
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
        matchEntryPath: undefined,
        matchAlternates: undefined,
      })),
    );
    setTransferMsg(null);
  };

  /** Read name + file list for a staged item without adding it. */
  const readTorrentMeta = async (
    item: StagingItem,
  ): Promise<StagingItem["meta"] | undefined> => {
    let r: AddTorrentResponse | undefined;
    if (item.kind === "server_path" && item.serverPath) {
      r = await API.uploadTorrentFromServerPath(item.serverPath, {
        list_only: true,
      });
    } else if (item.kind === "file" && item.file) {
      r = await API.uploadTorrent(item.file, { list_only: true });
    } else if (item.kind === "torrent_bytes" && item.bytes) {
      const file = new File(
        [
          item.bytes.buffer.slice(
            item.bytes.byteOffset,
            item.bytes.byteOffset + item.bytes.byteLength,
          ) as ArrayBuffer,
        ],
        "t.torrent",
      );
      r = await API.uploadTorrent(file, { list_only: true });
    }
    // Magnets would need a (possibly very slow) metadata resolve; they fall
    // back to name-only matching.
    if (!r) return undefined;
    return {
      name: r.details.name ?? undefined,
      files: r.details.files
        .filter((f) => !f.attributes?.padding)
        .map((f) => ({ components: f.components, length: f.length })),
    };
  };

  const onTransferFolders = async (selectedPaths: string[]) => {
    if (!selectedPaths.length) return;
    setTransferBusy(true);
    setTransferMsg("Scanning folders…");
    try {
      const candidates: TransferCandidate[] = [];
      const seen = new Set<string>();
      const discoveredTorrents: StagingItem[] = [];
      const errors: string[] = [];
      const toChild = (e: FsEntry): TransferCandidateChild => ({
        name: stripPartialSuffix(e.name),
        isDir: e.is_dir,
        size: e.size,
        partial: !!e.partial_of,
      });

      // Breadth-first scan: the selected folders, their subfolders, and so on
      // (completed data is often sorted into category folders, e.g.
      // Complete/Movies/<torrent>). Each listed folder is a candidate; files
      // are candidates for single-file torrents.
      type ScanDir = { path: string; name: string; depth: number };
      let level: ScanDir[] = selectedPaths.map((p) => ({
        path: p,
        name: p.split(/[/\\]/).pop() || p,
        depth: 0,
      }));
      let scanned = 0;
      let scanTruncated = false;
      while (level.length > 0) {
        const room = MAX_TRANSFER_SCAN_DIRS - scanned;
        if (room <= 0) {
          scanTruncated = true;
          break;
        }
        if (level.length > room) scanTruncated = true;
        const batch = level.slice(0, room);
        const nextLevel: ScanDir[] = [];
        await mapLimit(batch, 8, async (d) => {
          try {
            const listing = await API.fsList(d.path);
            if (seen.has(`dir:${listing.path}`)) return;
            seen.add(`dir:${listing.path}`);
            candidates.push({
              entryPath: listing.path,
              path: listing.path,
              name: d.name,
              kind: "dir",
              isRoot: d.depth === 0,
              children: listing.entries.map(toChild),
            });
            for (const e of listing.entries) {
              if (e.is_torrent) {
                if (d.depth === 0) {
                  discoveredTorrents.push({
                    id: nextId(),
                    label: e.name.replace(/\.torrent$/i, ""),
                    source: "transfer",
                    status: "ready",
                    kind: "server_path",
                    serverPath: e.path,
                  });
                }
                continue;
              }
              if (seen.has(e.path)) continue;
              seen.add(e.path);
              if (e.is_dir) {
                if (d.depth < MAX_TRANSFER_SCAN_DEPTH) {
                  nextLevel.push({
                    path: e.path,
                    name: e.name,
                    depth: d.depth + 1,
                  });
                }
              } else {
                // Single-file torrent data: the containing folder is the
                // output folder.
                candidates.push({
                  entryPath: e.path,
                  path: listing.path,
                  name: stripPartialSuffix(e.name),
                  kind: "file",
                  size: e.size,
                  partial: !!e.partial_of,
                });
              }
            }
          } catch (err: unknown) {
            errors.push(`${d.name}: ${errText(err)}`);
          } finally {
            scanned++;
            if (scanned % 25 === 0)
              setTransferMsg(`Scanning folders… ${scanned}`);
          }
        });
        level = nextLevel;
      }
      if (scanTruncated) {
        errors.push(
          `scan stopped after ${MAX_TRANSFER_SCAN_DIRS} folders — select narrower folders`,
        );
      }

      // Merge newly discovered .torrent paths, then read metadata for every
      // staged torrent that doesn't have it yet.
      const key = (i: StagingItem) =>
        i.serverPath ? `srv:${i.serverPath}` : i.text ? `text:${i.text}` : i.id;
      const current = queueRef.current;
      const known = new Set(current.map(key));
      const newItems = discoveredTorrents.filter((d) => {
        const k = key(d);
        if (known.has(k)) return false;
        known.add(k);
        return true;
      });
      const needMeta = [...current, ...newItems].filter((i) => !i.meta);
      const metaById = new Map<string, StagingItem["meta"]>();
      let metaDone = 0;
      await mapLimit(needMeta, 4, async (i) => {
        try {
          const m = await readTorrentMeta(i);
          if (m) metaById.set(i.id, m);
        } catch (err: unknown) {
          errors.push(`${i.label}: ${errText(err)}`);
        }
        metaDone++;
        setTransferMsg(
          `Reading torrent metadata… ${metaDone}/${needMeta.length}`,
        );
      });

      setQueue((prev) => {
        const prevKeys = new Set(prev.map(key));
        const merged = [
          ...prev,
          ...newItems.filter((n) => !prevKeys.has(key(n))),
        ].map((i) =>
          metaById.has(i.id) ? { ...i, meta: metaById.get(i.id) } : i,
        );

        const hints = merged.map((i) => ({
          id: i.id,
          label: i.label,
          name: i.meta?.name,
          files: i.meta?.files,
        }));
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
            matchEntryPath: m.entryPath,
            matchStatus: m.status,
            matchConfidence: m.confidence,
            matchReason: m.reason,
            matchAlternates: m.alternates?.map((a) => a.entryPath),
          };
        });
        setTransferMsg(
          `Transfer: ${selectedPaths.length} folder${selectedPaths.length === 1 ? "" : "s"} scanned (${candidates.length} candidates) · ` +
            `${matched} matched · ${ambiguous} ambiguous · ${unmatched} unmatched` +
            (newItems.length ? ` · ${newItems.length} .torrent found` : "") +
            (errors.length
              ? ` · ${errors.length} error(s): ${errors.slice(0, 3).join("; ")}`
              : ""),
        );
        return next;
      });
    } finally {
      setTransferBusy(false);
    }
  };

  const setItem = (id: string, patch: Partial<StagingItem>) =>
    setQueue((prev) => prev.map((i) => (i.id === id ? { ...i, ...patch } : i)));

  const startImport = async () => {
    cancelRef.current = { cancelled: false };
    setRunning(true);
    setProgress(null);

    const pasted: StagingItem[] = (transferMode ? [] : pastedNotQueued).map(
      (s) => ({
        id: nextId(),
        label: displayNameForSource(s.value),
        source: "url",
        status: "pending",
        kind: s.kind,
        text: s.value,
      }),
    );
    if (pasted.length > 0) setPasteText("");

    const workItems = [...queue.filter(isStartable), ...pasted];

    // Mark them queued in UI
    setQueue((prev) => [
      ...prev.map((i) =>
        workItems.some((w) => w.id === i.id)
          ? {
              ...i,
              status: "pending" as const,
              error: undefined,
              startedAt: undefined,
            }
          : i,
      ),
      ...pasted,
    ]);

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
              // The worker refines "running" into "resolving" for magnets.
              const status =
                st.status === "running" && item.status === "resolving"
                  ? "resolving"
                  : (st.status as StagingItem["status"]);
              // User-initiated Stop is a cancel, not a failure.
              if (st.status === "error" && st.error === STOPPED) {
                return { ...item, status: "cancelled", error: undefined };
              }
              return { ...item, status, error: st.error };
            });
          });
        },
        worker: async (item) => {
          const adopt = transferMode && item.matchStatus === "matched";
          const itemOpts: ItemOpts = adopt
            ? {
                // Reuse the other client's files in place: the server renames
                // qBittorrent `.!qB` partials and refuses files larger than
                // the torrent expects, then hash-checks before writing.
                overwrite: true,
                output_folder: item.matchedPath,
                adopt_foreign_incomplete: "qbit",
              }
            : {
                overwrite,
                output_folder: outputFolder.trim() || undefined,
              };
          const magnet = isMagnetItem(item) && !item.file && !item.bytes;
          const timeoutMs = magnet ? MAGNET_TIMEOUT_MS : OTHER_TIMEOUT_MS;
          const ctrl = new AbortController();
          let timedOut = false;
          const timer = window.setTimeout(() => {
            timedOut = true;
            ctrl.abort();
          }, timeoutMs);
          abortersRef.current.set(item.id, ctrl);
          setItem(item.id, {
            status: magnet ? "resolving" : "running",
            startedAt: Date.now(),
          });
          const init = { signal: ctrl.signal };
          try {
            await addOne(item, itemOpts, init);
          } catch (e) {
            if (timedOut) {
              throw new Error(
                magnet
                  ? `Timed out after ${formatDuration(timeoutMs)} waiting for torrent metadata — no peer sent it. The magnet may be dead or poorly seeded; retry later or use a .torrent file.`
                  : `Timed out after ${formatDuration(timeoutMs)} waiting for the server.`,
              );
            }
            if (ctrl.signal.aborted) throw new Error(STOPPED);
            throw e;
          } finally {
            window.clearTimeout(timer);
            abortersRef.current.delete(item.id);
          }
        },
      });
      setProgress(result);
      if (result.ok > 0) {
        refreshTorrents();
      }
    } finally {
      abortAll();
      setRunning(false);
    }
  };

  const addOne = async (
    item: StagingItem,
    itemOpts: ItemOpts,
    init: { signal: AbortSignal },
  ) => {
    if (item.kind === "server_path" && item.serverPath) {
      await API.uploadTorrentFromServerPath(item.serverPath, itemOpts, init);
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
      await API.uploadTorrent(file, itemOpts, init);
    } else if (item.file) {
      await API.uploadTorrent(item.file, itemOpts, init);
    } else if (item.text) {
      await API.uploadTorrent(item.text, itemOpts, init);
    } else {
      throw new Error("nothing to upload");
    }
  };

  const stopQueue = () => {
    cancelRef.current.cancelled = true;
    abortAll();
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
                    : `${detectedUrls.length} source${detectedUrls.length === 1 ? "" : "s"} detected — press Add to start.`}
              </div>
              {transferMode && (
                <Button
                  size="sm"
                  variant="primary"
                  disabled={running || detectedUrls.length === 0}
                  onClick={addDetectedUrls}
                >
                  Add to queue
                </Button>
              )}
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
              onRetry={(id) =>
                setItem(id, {
                  status: "ready",
                  error: undefined,
                  startedAt: undefined,
                })
              }
            />
          </>
        )}

        <div className="mt-3">
          <button
            type="button"
            className="flex items-center gap-1 text-sm text-secondary hover:text-text cursor-pointer"
            aria-expanded={advancedOpen}
            onClick={() => setAdvancedOpen((v) => !v)}
          >
            <span
              className={`inline-block transition-transform ${advancedOpen ? "rotate-90" : ""}`}
            >
              ▸
            </span>
            Advanced
            {!advancedOpen && advancedSummary.length > 0 && (
              <span className="text-primary">
                · {advancedSummary.join(" · ")}
              </span>
            )}
          </button>
          {advancedOpen && (
            <div className="mt-2 flex flex-col gap-3 border-l-2 border-divider pl-3">
              <div className="flex flex-col gap-2">
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
                      Select folders that already contain downloads (or a parent
                      of many). We&apos;ll fuzzy-match them to the queue and set
                      each item&apos;s output folder.
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
                      <div className="text-sm text-secondary mt-2">
                        {transferMsg}
                      </div>
                    )}
                    {transferBusy && (
                      <div className="text-sm text-secondary mt-1">
                        Working…
                      </div>
                    )}
                  </div>
                )}
              </div>
              <div>
                <FormInput
                  label="Output folder (optional)"
                  name="add_output_folder"
                  value={outputFolder}
                  disabled={running}
                  onChange={(e) => setOutputFolder(e.target.value)}
                  placeholder="Leave empty for session default"
                />
              </div>

              <div>
                <FormCheckbox
                  name="add_overwrite"
                  label="Overwrite existing files on disk"
                  checked={overwrite}
                  disabled={running}
                  onChange={() => setOverwrite(!overwrite)}
                />
              </div>

              <div className="flex flex-col gap-1">
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
            </div>
          )}
        </div>

        {progress && (
          <div className="mt-3 mb-2 flex flex-col gap-1">
            <div className="text-sm text-secondary">
              {[
                `${progress.done}/${progress.total} done`,
                resolvingCount > 0
                  ? `${resolvingCount} resolving metadata`
                  : null,
                inFlight.length - resolvingCount > 0
                  ? `${inFlight.length - resolvingCount} adding`
                  : null,
                `${progress.ok} added`,
                progress.failed > 0 ? `${progress.failed} failed` : null,
                progress.cancelled ? `${progress.cancelled} cancelled` : null,
              ]
                .filter(Boolean)
                .join(" · ")}
            </div>
            <ProgressBar
              now={pct}
              label=""
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
