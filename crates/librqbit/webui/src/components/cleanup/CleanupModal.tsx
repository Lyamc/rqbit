import { useCallback, useContext, useEffect, useMemo, useState } from "react";
import { APIContext } from "../../context";
import {
  CleanupApplyOutcome,
  CleanupRoot,
  CleanupRootsResponse,
  CleanupScan,
  QuarantineBatch,
} from "../../api-types";
import { Modal } from "../modal/Modal";
import { ModalBody } from "../modal/ModalBody";
import { ModalFooter } from "../modal/ModalFooter";
import { Button } from "../buttons/Button";
import { Spinner } from "../Spinner";
import { FilesystemBrowser } from "../filesystem/FilesystemBrowser";
import { formatBytes } from "../../helper/formatBytes";
import {
  formatAge,
  parseScanHours,
  relPath,
  selectionSummary,
} from "../../helper/cleanup";

const kindLabel: Record<CleanupRoot["kind"], string> = {
  download: "download folder",
  move: "completion Move folder",
  organize: "organize folder",
  custom: "added folder",
};

const errText = (e: any): string =>
  e?.text ?? e?.message ?? (typeof e === "string" ? e : JSON.stringify(e));

export const CleanupModal: React.FC<{ isOpen: boolean; onClose: () => void }> = ({
  isOpen,
  onClose,
}) => {
  const API = useContext(APIContext);
  const [info, setInfo] = useState<CleanupRootsResponse | null>(null);
  const [roots, setRoots] = useState<CleanupRoot[]>([]);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [minAge, setMinAge] = useState("60");
  const [browsing, setBrowsing] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [scan, setScan] = useState<CleanupScan | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [action, setAction] = useState<"quarantine" | "delete">("quarantine");
  const [confirming, setConfirming] = useState(false);
  const [understood, setUnderstood] = useState(false);
  const [applying, setApplying] = useState(false);
  const [outcome, setOutcome] = useState<CleanupApplyOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [batches, setBatches] = useState<QuarantineBatch[]>([]);
  const [batchBusy, setBatchBusy] = useState<string | null>(null);
  const [purgeAsk, setPurgeAsk] = useState<string | null>(null);
  const [hours, setHours] = useState("");
  const [hoursMsg, setHoursMsg] = useState<string | null>(null);
  const now = Math.floor(Date.now() / 1000);

  const loadQuarantine = useCallback(() => {
    API.cleanupQuarantine?.()
      .then((q) => setBatches(q.batches))
      .catch(() => {});
  }, [API]);

  useEffect(() => {
    if (!isOpen) return;
    setError(null);
    setOutcome(null);
    API.cleanupRoots?.()
      .then((r) => {
        setInfo(r);
        setRoots(r.roots);
        setTicked(new Set(r.roots.filter((x) => x.default_on).map((x) => x.path)));
        setMinAge(String(r.min_age_minutes));
        setHours(r.scan_hours ? String(r.scan_hours) : "");
      })
      .catch((e) => setError(errText(e)));
    loadQuarantine();
  }, [isOpen]);

  const runScan = () => {
    const age = parseInt(minAge, 10);
    setScanning(true);
    setError(null);
    setOutcome(null);
    setConfirming(false);
    API.cleanupScan?.(
      [...ticked],
      Number.isFinite(age) && age >= 0 ? age : undefined,
    )
      .then((s) => {
        setScan(s);
        setSelected(new Set());
      })
      .catch((e) => setError(errText(e)))
      .finally(() => setScanning(false));
  };

  const showLatest = () => {
    API.cleanupScanResult?.(info?.latest_scan?.scan_id)
      .then((s) => {
        setScan(s);
        setSelected(new Set());
      })
      .catch((e) => setError(errText(e)));
  };

  const sel = useMemo(
    () => selectionSummary(scan?.items ?? [], selected),
    [scan, selected],
  );

  const apply = () => {
    if (!scan) return;
    setApplying(true);
    setError(null);
    API.cleanupApply?.(scan.scan_id, [...selected], action, action === "delete")
      .then((o) => {
        setOutcome(o);
        setConfirming(false);
        setUnderstood(false);
        const done = new Set(o.results.filter((r) => r.ok).map((r) => r.id));
        const items = scan.items.filter((i) => !done.has(i.id));
        setScan({ ...scan, items, total_bytes: items.reduce((a, i) => a + i.size, 0) });
        setSelected(new Set());
        loadQuarantine();
      })
      .catch((e) => setError(errText(e)))
      .finally(() => setApplying(false));
  };

  const restore = (b: QuarantineBatch) => {
    setBatchBusy(b.id);
    API.cleanupRestore?.(b.id)
      .then((r) => {
        const bad = r.results.filter((x) => !x.ok);
        setError(
          bad.length
            ? `Restored ${r.results.length - bad.length}; not restored: ${bad
                .map((x) => `${x.path} (${x.error})`)
                .join(", ")}`
            : null,
        );
        loadQuarantine();
      })
      .catch((e) => setError(errText(e)))
      .finally(() => setBatchBusy(null));
  };

  const purge = (id: string) => {
    setBatchBusy(id);
    API.cleanupPurge?.(id)
      .then(() => {
        setPurgeAsk(null);
        loadQuarantine();
      })
      .catch((e) => setError(errText(e)))
      .finally(() => setBatchBusy(null));
  };

  const saveHours = async () => {
    const h = parseScanHours(hours);
    if (h === "invalid") {
      setHoursMsg("Enter whole hours, or leave empty for off.");
      return;
    }
    try {
      const prefs = await API.getPreferences();
      await API.setPreferences({ ...prefs, cleanup_scan_hours: h });
      setHoursMsg(h ? `Saved: scans every ${h} h (report only).` : "Saved: off.");
    } catch (e) {
      setHoursMsg(errText(e));
    }
  };

  const toggleRoot = (p: string) => {
    const n = new Set(ticked);
    n.has(p) ? n.delete(p) : n.add(p);
    setTicked(n);
  };

  const allTicked = !!scan && scan.items.length > 0 && selected.size === scan.items.length;

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title="Clean up orphaned downloads"
      className="sm:max-w-5xl"
    >
      <ModalBody>
        <div className="flex flex-col gap-3 text-sm" data-testid="cleanup-modal">
          <p className="text-secondary">
            Finds files and folders in your download locations that no torrent
            in rqbit uses. Scanning changes nothing. Hidden/system files,
            symlinks and anything changed recently are never touched; every
            item is checked again before it is moved or deleted.
          </p>

          <section>
            <h3 className="font-semibold mb-1">Folders to scan</h3>
            {!info && !error && <Spinner />}
            <div className="flex flex-col gap-1">
              {roots.map((r) => (
                <label key={r.path} className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={ticked.has(r.path)}
                    disabled={!r.exists}
                    onChange={() => toggleRoot(r.path)}
                  />
                  <span className="font-mono">{r.path}</span>
                  <span className="text-xs text-tertiary">
                    {kindLabel[r.kind]}
                    {!r.exists && " · missing"}
                    {(r.kind === "move" || r.kind === "organize") &&
                      " · library: files here are often kept on purpose"}
                  </span>
                </label>
              ))}
            </div>
            <div className="flex flex-wrap items-center gap-2 mt-2">
              <Button size="sm" variant="secondary" onClick={() => setBrowsing(!browsing)}>
                {browsing ? "Close folder picker" : "Add folder…"}
              </Button>
              <span>Ignore anything changed in the last</span>
              <input
                className="w-16 px-1 py-0.5 border border-divider rounded bg-surface"
                value={minAge}
                onChange={(e) => setMinAge(e.target.value)}
                inputMode="numeric"
              />
              <span>minutes</span>
              <Button
                variant="primary"
                onClick={runScan}
                disabled={scanning || ticked.size === 0}
              >
                {scanning ? "Scanning…" : "Scan (dry run)"}
              </Button>
              {info?.latest_scan && !scan && (
                <button className="text-primary underline text-xs" onClick={showLatest}>
                  {info.latest_scan.scheduled ? "Scheduled scan" : "Last scan"}{" "}
                  {new Date(info.latest_scan.time * 1000).toLocaleString()}:{" "}
                  {info.latest_scan.items} item(s), {formatBytes(info.latest_scan.total_bytes)} — show
                </button>
              )}
            </div>
            {browsing && (
              <div className="mt-2 border border-divider rounded p-2">
                <FilesystemBrowser
                  mode="select-directory"
                  confirmLabel="Add this folder"
                  onCancel={() => setBrowsing(false)}
                  onConfirm={(paths) => {
                    const p = paths[0];
                    if (p && !roots.some((r) => r.path === p)) {
                      setRoots([...roots, { path: p, kind: "custom", default_on: true, exists: true }]);
                    }
                    if (p) setTicked(new Set([...ticked, p]));
                    setBrowsing(false);
                  }}
                />
                <p className="text-xs text-tertiary mt-1">
                  Must be inside the download folder, a completion folder, or a server browse root.
                </p>
              </div>
            )}
          </section>

          {error && (
            <div className="text-error border border-error/40 rounded p-2" data-testid="cleanup-error">
              {error}
            </div>
          )}

          {outcome && (
            <div
              className={`rounded p-2 border ${outcome.failed ? "border-warning-bg" : "border-success-bg"}`}
              data-testid="cleanup-outcome"
            >
              {outcome.action === "delete" ? "Deleted" : "Moved to quarantine"} {outcome.ok} item(s),{" "}
              {formatBytes(outcome.bytes)}.
              {outcome.failed > 0 && (
                <ul className="list-disc ml-5 mt-1 text-xs">
                  {outcome.results
                    .filter((r) => !r.ok)
                    .map((r) => (
                      <li key={r.path}>
                        <span className="font-mono">{r.path}</span>: {r.error}
                      </li>
                    ))}
                </ul>
              )}
            </div>
          )}

          {scan && (
            <section data-testid="cleanup-results">
              <h3 className="font-semibold mb-1">
                {scan.items.length === 0
                  ? "Nothing orphaned found"
                  : `${scan.items.length} orphaned item(s), ${formatBytes(scan.total_bytes)}`}
                <span className="font-normal text-xs text-tertiary">
                  {" "}
                  · {scan.scheduled ? "scheduled scan" : "dry run"}{" "}
                  {new Date(scan.time * 1000).toLocaleString()} · checked against{" "}
                  {scan.torrents_checked} torrent(s) · skipped {scan.skipped.recent} recent,{" "}
                  {scan.skipped.hidden} hidden/system, {scan.skipped.symlinks} symlink(s)
                  {scan.skipped.truncated && " · list truncated"}
                </span>
              </h3>
              {scan.skipped.errors.length > 0 && (
                <div className="text-xs text-warning mb-1">
                  Couldn't read: {scan.skipped.errors.slice(0, 3).join("; ")}
                </div>
              )}
              {scan.items.length > 0 && (
                <div className="max-h-[40vh] overflow-auto border border-divider rounded">
                  <table className="w-full text-xs">
                    <thead className="sticky top-0 bg-surface-raised">
                      <tr className="text-left text-tertiary">
                        <th className="p-1 w-6">
                          <input
                            type="checkbox"
                            checked={allTicked}
                            onChange={() =>
                              setSelected(allTicked ? new Set() : new Set(scan.items.map((i) => i.id)))
                            }
                            aria-label="Select all"
                          />
                        </th>
                        <th className="p-1">Path</th>
                        <th className="p-1 text-right">Size</th>
                        <th className="p-1">Modified</th>
                        <th className="p-1">Why</th>
                      </tr>
                    </thead>
                    <tbody>
                      {scan.items.map((it) => (
                        <tr
                          key={it.id}
                          className={`border-t border-divider ${selected.has(it.id) ? "bg-primary/10" : ""}`}
                        >
                          <td className="p-1">
                            <input
                              type="checkbox"
                              checked={selected.has(it.id)}
                              onChange={() => {
                                const n = new Set(selected);
                                n.has(it.id) ? n.delete(it.id) : n.add(it.id);
                                setSelected(n);
                              }}
                            />
                          </td>
                          <td className="p-1 font-mono break-all" title={it.path}>
                            {it.kind === "dir" ? "📁 " : ""}
                            {relPath(it.path, it.root)}
                            {scan.roots.length > 1 && (
                              <span className="text-tertiary"> ({it.root})</span>
                            )}
                          </td>
                          <td className="p-1 text-right whitespace-nowrap">
                            {formatBytes(it.size)}
                            {it.kind === "dir" && (
                              <span className="text-tertiary"> · {it.files} files</span>
                            )}
                          </td>
                          <td
                            className="p-1 whitespace-nowrap"
                            title={it.mtime ? new Date(it.mtime * 1000).toLocaleString() : ""}
                          >
                            {formatAge(it.mtime, now)}
                          </td>
                          <td className="p-1 text-secondary">{it.reason}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {scan.items.length > 0 && (
                <div className="flex flex-wrap items-center gap-3 mt-2">
                  <label className="flex items-center gap-1">
                    <input
                      type="radio"
                      checked={action === "quarantine"}
                      onChange={() => {
                        setAction("quarantine");
                        setConfirming(false);
                      }}
                    />
                    Move to quarantine (can be restored)
                  </label>
                  <label className="flex items-center gap-1">
                    <input
                      type="radio"
                      checked={action === "delete"}
                      onChange={() => {
                        setAction("delete");
                        setConfirming(false);
                      }}
                    />
                    Delete permanently
                  </label>
                  <Button
                    variant={action === "delete" ? "danger" : "primary"}
                    disabled={sel.count === 0 || applying}
                    onClick={() => setConfirming(true)}
                  >
                    {action === "delete" ? "Delete" : "Quarantine"} {sel.count} item(s) ({formatBytes(sel.bytes)})…
                  </Button>
                </div>
              )}
              {confirming && (
                <div
                  className={`mt-2 p-2 rounded border ${action === "delete" ? "border-error" : "border-divider"}`}
                  data-testid="cleanup-confirm"
                >
                  {action === "delete" ? (
                    <>
                      <p className="text-error font-semibold">
                        Permanently delete {sel.count} item(s), {sel.files} file(s), {formatBytes(sel.bytes)}? This can't be undone.
                      </p>
                      <label className="flex items-center gap-1 mt-1">
                        <input type="checkbox" checked={understood} onChange={() => setUnderstood(!understood)} />
                        I understand these files will be gone
                      </label>
                    </>
                  ) : (
                    <p>
                      Move {sel.count} item(s), {formatBytes(sel.bytes)} into{" "}
                      <span className="font-mono">.rqbit-quarantine</span> inside each scanned folder?
                      You can restore them below, or delete them later.
                    </p>
                  )}
                  <div className="flex gap-2 mt-2">
                    <Button variant="cancel" onClick={() => setConfirming(false)}>
                      Cancel
                    </Button>
                    <Button
                      variant={action === "delete" ? "danger" : "primary"}
                      disabled={applying || (action === "delete" && !understood)}
                      onClick={apply}
                    >
                      {applying ? "Working…" : action === "delete" ? "Delete permanently" : "Move to quarantine"}
                    </Button>
                  </div>
                </div>
              )}
            </section>
          )}

          <section>
            <h3 className="font-semibold mb-1">Quarantine</h3>
            {batches.length === 0 ? (
              <p className="text-tertiary">Empty.</p>
            ) : (
              <div className="flex flex-col gap-1">
                {batches.map((b) => {
                  const bytes = b.items.reduce((a, i) => a + i.size, 0);
                  return (
                    <div key={b.id} className="flex flex-wrap items-center gap-2 border-b border-divider pb-1">
                      <span>{new Date(b.created * 1000).toLocaleString()}</span>
                      <span className="text-tertiary">
                        {b.items.length} item(s), {formatBytes(bytes)} in{" "}
                        <span className="font-mono">{b.dir}</span>
                      </span>
                      <Button size="sm" variant="secondary" disabled={batchBusy === b.id} onClick={() => restore(b)}>
                        Restore
                      </Button>
                      {purgeAsk === b.id ? (
                        <>
                          <span className="text-error">Delete these permanently?</span>
                          <Button size="sm" variant="danger" disabled={batchBusy === b.id} onClick={() => purge(b.id)}>
                            Delete permanently
                          </Button>
                          <Button size="sm" variant="cancel" onClick={() => setPurgeAsk(null)}>
                            Cancel
                          </Button>
                        </>
                      ) : (
                        <Button size="sm" variant="secondary" onClick={() => setPurgeAsk(b.id)}>
                          Delete…
                        </Button>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </section>

          <section className="flex flex-wrap items-center gap-2">
            <h3 className="font-semibold">Scheduled scan</h3>
            <span>every</span>
            <input
              className="w-16 px-1 py-0.5 border border-divider rounded bg-surface"
              value={hours}
              placeholder="off"
              onChange={(e) => {
                setHours(e.target.value);
                setHoursMsg(null);
              }}
            />
            <span>hours — reports only (Events + here), never moves or deletes.</span>
            <Button size="sm" variant="secondary" onClick={saveHours}>
              Save
            </Button>
            {hoursMsg && <span className="text-xs text-tertiary">{hoursMsg}</span>}
          </section>
        </div>
      </ModalBody>
      <ModalFooter>
        <Button variant="cancel" onClick={onClose}>
          Close
        </Button>
      </ModalFooter>
    </Modal>
  );
};
