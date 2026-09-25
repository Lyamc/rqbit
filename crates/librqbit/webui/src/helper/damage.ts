import { DamageStats, RepairSummary } from "../api-types";
import { formatBytes } from "./formatBytes";

export const hasDamagedFiles = (d?: DamageStats | null): boolean =>
  !!d && d.damaged_files.length > 0;

/** Pieces held back / given up after I/O errors, or files whose auto repair gave up. */
export const hasRecoveryIssues = (d?: DamageStats | null): boolean =>
  !!d && (!!d.recovery || !!d.needs_attention);

export const formatDuration = (secs: number): string => {
  if (secs < 60) return `${Math.max(0, Math.round(secs))}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  const h = Math.floor(secs / 3600);
  const m = Math.round((secs % 3600) / 60);
  return m ? `${h}h ${m}m` : `${h}h`;
};

/** "3 piece(s) waiting to retry (attempt 2/8), next retry in 4m; 1 needs attention" */
export const recoveryText = (d?: DamageStats | null): string | null => {
  const r = d?.recovery;
  const parts: string[] = [];
  if (r && r.pieces_waiting > 0) {
    let t = `${r.pieces_waiting} piece(s) held back after I/O errors (attempt ${r.max_piece_attempts}/${r.max_attempts})`;
    if (r.next_retry_in_secs !== undefined)
      t += `, next retry in ${formatDuration(r.next_retry_in_secs)}`;
    parts.push(t);
  }
  if (r && r.pieces_needing_attention > 0) {
    parts.push(
      `${r.pieces_needing_attention} piece(s) need attention (automatic retries stopped after ${r.max_attempts} failures)`,
    );
  }
  const files = d?.damaged_files ?? [];
  const gaveUp = files.filter((f) => f.needs_attention).length;
  if (gaveUp > 0) {
    parts.push(`automatic repair gave up on ${gaveUp} file(s) — needs attention`);
  } else {
    const next = files
      .filter((f) => f.next_auto_repair_in_secs !== undefined)
      .map((f) => f.next_auto_repair_in_secs as number);
    const attempts = Math.max(0, ...files.map((f) => f.auto_repair_attempts ?? 0));
    if (attempts > 0 && next.length > 0) {
      parts.push(
        `auto repair attempt ${attempts}, next in ${formatDuration(Math.min(...next))}`,
      );
    }
  }
  return parts.length ? parts.join("; ") : null;
};

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
  const rec = recoveryText(d);
  const attention = d.needs_attention ? "Needs attention: " : "";
  if (d.damaged_files.length > 0) {
    return `${attention}${d.damaged_files.length} damaged file(s) (disk I/O errors) — use Fix errors → Repair${rec ? ` (${rec})` : ""}`;
  }
  if (rec) {
    return `${attention}${rec} — Fix errors retries now`;
  }
  if (r?.state === "failed") {
    return `Repair failed: ${r.error ?? "unknown error"}`;
  }
  if (r?.state === "done" && r.summary) {
    return `Repaired: ${repairSummaryText(r.summary)}`;
  }
  return null;
};
