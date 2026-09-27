/** "1d 2h", "45m", "30s" (0 → "0s"). */
export function formatDuration(secs: number): string {
  secs = Math.max(0, Math.round(secs));
  if (secs === 0) return "0s";
  const parts: string[] = [];
  const units: [string, number][] = [
    ["d", 86400],
    ["h", 3600],
    ["m", 60],
    ["s", 1],
  ];
  for (const [u, n] of units) {
    if (secs >= n) {
      parts.push(`${Math.floor(secs / n)}${u}`);
      secs %= n;
    }
    if (parts.length === 2) break;
  }
  return parts.join(" ");
}

/**
 * Parse "90", "90s", "15m", "2h 30m", "1.5h", "3d". A bare number is
 * `bareUnit` (default minutes). Returns null for empty/invalid input.
 */
export function parseDuration(
  input: string,
  bareUnit: "s" | "m" | "h" = "m",
): number | null {
  const s = input.trim().toLowerCase();
  if (!s) return null;
  const mult: Record<string, number> = { s: 1, m: 60, h: 3600, d: 86400 };
  if (/^\d+(\.\d+)?$/.test(s)) return Math.round(parseFloat(s) * mult[bareUnit]);
  const re = /(\d+(?:\.\d+)?)\s*([smhd])[a-z]*/g;
  let total = 0;
  let consumed = "";
  let m: RegExpExecArray | null;
  while ((m = re.exec(s))) {
    total += parseFloat(m[1]) * mult[m[2]];
    consumed += m[0];
  }
  if (consumed.replace(/\s/g, "") !== s.replace(/\s/g, "")) return null;
  return Math.round(total);
}

/**
 * Parse "500 MB", "1.5GB", "2 TiB", "1024" / "10 bytes". Units are binary
 * (1 KB = 1024 bytes), matching how sizes are displayed everywhere.
 */
export function parseSize(input: string): number | null {
  const s = input.trim().toLowerCase().replace(/\s+/g, "");
  if (!s) return null;
  const m = /^(\d+(?:\.\d+)?)(?:([kmgt])i?b?|b|bytes?)?$/.exec(s);
  if (!m) return null;
  const exp = m[2] ? { k: 1, m: 2, g: 3, t: 4 }[m[2] as "k"]! : 0;
  return Math.round(parseFloat(m[1]) * Math.pow(1024, exp));
}
