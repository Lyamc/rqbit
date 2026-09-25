import { DamageStats, RepairSummary } from "../api-types";
import { formatBytes } from "./formatBytes";

export const hasDamagedFiles = (d?: DamageStats | null): boolean =>
  !!d && d.damaged_files.length > 0;

export const isRepairRunning = (d?: DamageStats | null): boolean =>
  d?.repair?.state === "running";

const METHOD_LABEL: Record<string, string> = {
  punch_hole: "punched out unreadable ranges",
  copy_replace: "copied and replaced the file",
  failed: "failed",
};

/** One-line human summary of a finished repair. */
export const repairSummaryText = (s: RepairSummary): string => {
  if (s.files_repaired === 0 && s.files_failed === 0) {
    return `No unreadable data found in ${s.files_scanned} file(s).`;
  }
  const methods = Array.from(
    new Set(s.files.map((f) => METHOD_LABEL[f.method] ?? f.method)),
  ).join(", ");
  let t = `${formatBytes(s.bytes_unreadable)} unreadable in ${s.files_repaired + s.files_failed} file(s); zeroed ${formatBytes(s.bytes_zeroed)} (${methods}); ${s.pieces_to_redownload} piece(s) to re-download`;
  if (s.pieces_invalidated > 0) {
    t += ` (${s.pieces_invalidated} previously verified)`;
  }
  if (s.files_failed > 0) {
    t += `; ${s.files_failed} file(s) could not be repaired`;
  }
  return t + ".";
};

/** Short status for list rows. Null when there's nothing to show. */
export const damageShortText = (d?: DamageStats | null): string | null => {
  if (!d) return null;
  const r = d.repair;
  if (r?.state === "running") {
    const pct = r.total_bytes
      ? Math.floor((r.scanned_bytes / r.total_bytes) * 100)
      : 0;
    return `Repairing damaged files… ${pct}% scanned (${r.files_done}/${r.files_total} files)`;
  }
  if (d.damaged_files.length > 0) {
    return `${d.damaged_files.length} damaged file(s) (disk I/O errors) — use Fix errors → Repair`;
  }
  if (r?.state === "failed") {
    return `Repair failed: ${r.error ?? "unknown error"}`;
  }
  if (r?.state === "done" && r.summary) {
    return `Repaired: ${repairSummaryText(r.summary)}`;
  }
  return null;
};
