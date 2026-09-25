import { useContext, useState } from "react";
import { FaWrench } from "react-icons/fa";
import { APIContext } from "../context";
import { ErrorDetails, TorrentListItem } from "../api-types";
import { useTorrentStore } from "../stores/torrentStore";
import { useErrorStore } from "../stores/errorStore";
import { formatBytes } from "../helper/formatBytes";
import {
  formatDuration,
  hasDamagedFiles,
  hasRecoveryIssues,
  isRepairRunning,
  recoveryText,
  repairSummaryText,
} from "../helper/damage";
import { Button } from "./buttons/Button";

/**
 * Shows files that rqbit could not read/write (e.g. unreadable extents on the
 * filesystem), the state of any repair, and a "Repair damaged files" action.
 */
export const DamagedFilesNotice: React.FC<{ torrent: TorrentListItem }> = ({
  torrent,
}) => {
  const API = useContext(APIContext);
  const refreshTorrents = useTorrentStore((s) => s.refreshTorrents);
  const setCloseableError = useErrorStore((s) => s.setCloseableError);
  const [starting, setStarting] = useState(false);

  const damage = torrent.stats?.damage;
  if (!damage) return null;
  const repair = damage.repair;
  const running = isRepairRunning(damage);
  const damaged = hasDamagedFiles(damage);
  const recovery = hasRecoveryIssues(damage);
  if (!damaged && !repair && !recovery) return null;
  const recText = recoveryText(damage);

  const fixNow = async () => {
    setStarting(true);
    try {
      await API.fixErrors(torrent.id);
      refreshTorrents();
    } catch (e) {
      setCloseableError({
        text: `Error fixing errors on torrent id=${torrent.id}`,
        details: e as ErrorDetails,
      });
    } finally {
      setStarting(false);
    }
  };

  const startRepair = async () => {
    if (!API.repairFiles) return;
    setStarting(true);
    try {
      await API.repairFiles(torrent.id);
      refreshTorrents();
    } catch (e) {
      setCloseableError({
        text: `Error starting repair of torrent id=${torrent.id}`,
        details: e as ErrorDetails,
      });
    } finally {
      setStarting(false);
    }
  };

  return (
    <div
      className="mt-2 p-2 rounded border border-warning-bg text-sm space-y-1"
      data-testid="damaged-files-notice"
    >
      {damaged && (
        <>
          <div className="font-medium text-warning">
            {damage.damaged_files.length} damaged file(s): data on disk can't
            be read or overwritten (I/O errors)
          </div>
          <ul className="text-secondary list-disc ml-5">
            {damage.damaged_files.slice(0, 5).map((f) => (
              <li key={f.file_id} className="truncate" title={f.last_error}>
                {f.path} — {f.errors} error(s)
                {f.eio ? ", EIO" : ""}
                {f.needs_attention
                  ? " — auto repair gave up (needs attention)"
                  : f.auto_repair_attempts
                    ? ` — auto repair attempts: ${f.auto_repair_attempts}${
                        f.next_auto_repair_in_secs !== undefined
                          ? `, next in ${formatDuration(f.next_auto_repair_in_secs)}`
                          : ""
                      }`
                    : ""}
              </li>
            ))}
            {damage.damaged_files.length > 5 && (
              <li>…and {damage.damaged_files.length - 5} more</li>
            )}
          </ul>
          <div className="text-tertiary">
            Redownloading can't fix this by itself. Repair scans every file of
            the torrent, punches out the unreadable ranges (or copies the file
            and replaces it if that isn't possible), then redownloads only the
            affected pieces. The torrent is paused while it runs.
          </div>
        </>
      )}
      {recText && (
        <div
          className={damage.needs_attention ? "text-error" : "text-secondary"}
          data-testid="recovery-status"
          title={(damage.recovery?.pieces ?? [])
            .map(
              (p) =>
                `piece ${p.piece}: ${p.attempts} attempt(s)${
                  p.needs_attention
                    ? ", needs attention"
                    : p.next_retry_in_secs !== undefined
                      ? `, next retry in ${formatDuration(p.next_retry_in_secs)}`
                      : ""
                } — ${p.last_error}`,
            )
            .join("\n")}
        >
          {damage.needs_attention ? "Needs attention: " : "Automatic recovery: "}
          {recText}
        </div>
      )}
      {running && repair && (
        <div className="text-secondary" data-testid="repair-progress">
          Repairing… scanned {formatBytes(repair.scanned_bytes)} of{" "}
          {formatBytes(repair.total_bytes)} ({repair.files_done}/
          {repair.files_total} files)
          {repair.current_file ? ` — ${repair.current_file}` : ""}
        </div>
      )}
      {!running && repair?.state === "done" && repair.summary && (
        <div className="text-success" data-testid="repair-result">
          Repair done: {repairSummaryText(repair.summary)}
        </div>
      )}
      {!running && repair?.state === "failed" && (
        <div className="text-error" data-testid="repair-result">
          Repair failed: {repair.error}
          {repair.summary ? ` — ${repairSummaryText(repair.summary)}` : ""}
        </div>
      )}
      {recovery && !damaged && !running && (
        <Button
          onClick={fixNow}
          disabled={starting}
          variant="secondary"
          size="sm"
        >
          <FaWrench className="w-2.5 h-2.5" />
          Retry now (Fix errors)
        </Button>
      )}
      {(damaged || repair?.state === "failed") && API.repairFiles && (
        <Button
          onClick={startRepair}
          disabled={starting || running}
          variant="secondary"
          size="sm"
        >
          <FaWrench className="w-2.5 h-2.5" />
          Repair damaged files
        </Button>
      )}
    </div>
  );
};
