import { CleanupItem } from "../api-types";

/** Path shown relative to its scan root ("Show/ep1.mkv"). */
export function relPath(path: string, root: string): string {
  if (path === root) return path;
  const r = root.endsWith("/") || root.endsWith("\\") ? root : root + "/";
  if (path.startsWith(r)) return path.slice(r.length);
  const rw = root.endsWith("\\") ? root : root + "\\";
  if (path.startsWith(rw)) return path.slice(rw.length);
  return path;
}

/** "5 min ago", "3 h ago", "12 d ago", "2 y ago" (floored). */
export function formatAge(mtimeSecs: number | null, nowSecs: number): string {
  if (mtimeSecs === null || mtimeSecs === undefined) return "—";
  const d = Math.max(0, nowSecs - mtimeSecs);
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  if (d < 365 * 86400) return `${Math.floor(d / 86400)} d ago`;
  return `${Math.floor(d / (365 * 86400))} y ago`;
}

export function selectionSummary(
  items: CleanupItem[],
  selected: Set<number>,
): { count: number; bytes: number; files: number } {
  let count = 0,
    bytes = 0,
    files = 0;
  for (const it of items) {
    if (selected.has(it.id)) {
      count++;
      bytes += it.size;
      files += it.files;
    }
  }
  return { count, bytes, files };
}

/** Parse the scheduled-scan hours field: empty / 0 = off (null). */
export function parseScanHours(v: string): number | null | "invalid" {
  const t = v.trim();
  if (t === "") return null;
  if (!/^\d+$/.test(t)) return "invalid";
  const n = parseInt(t, 10);
  return n === 0 ? null : n;
}
