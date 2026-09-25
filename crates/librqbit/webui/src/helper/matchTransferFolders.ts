/** Fuzzy-match download folders from another client onto torrents being added. */

export type TransferMatchStatus = "matched" | "unmatched" | "ambiguous";

/** qBittorrent appends this to incomplete files when that option is on. */
export const QBIT_PARTIAL_SUFFIX = ".!qB";

/** `Movie.mkv.!qB` → `Movie.mkv`; other names unchanged. */
export const stripPartialSuffix = (name: string): string =>
  name.endsWith(QBIT_PARTIAL_SUFFIX) && name.length > QBIT_PARTIAL_SUFFIX.length
    ? name.slice(0, -QBIT_PARTIAL_SUFFIX.length)
    : name;

export type TransferCandidateChild = {
  /** Basename with any `.!qB` suffix removed. */
  name: string;
  isDir: boolean;
  size?: number;
  partial?: boolean;
};

export type TransferCandidate = {
  /** Server path of the folder or file this candidate represents. */
  entryPath: string;
  /** Output folder rqbit should use if matched: the folder itself for a
   *  directory, the containing folder for a single file. */
  path: string;
  /** Basename (for files, with any `.!qB` suffix removed). */
  name: string;
  kind: "dir" | "file";
  /** Files only. */
  size?: number;
  /** Files only: another client's in-progress file (`.!qB`). */
  partial?: boolean;
  /** Directories only: direct children, when listed. */
  children?: TransferCandidateChild[];
  /** A folder the user selected (e.g. qBittorrent's Complete/Incomplete). */
  isRoot?: boolean;
};

export type TransferTorrentFile = {
  /** Path components relative to the torrent's output folder. */
  components: string[];
  length: number;
};

export type TransferTorrentHint = {
  id: string;
  /** Display label (fallback when metadata is unknown, e.g. magnets). */
  label: string;
  /** Torrent name from metadata. */
  name?: string;
  /** Non-padding files from metadata. */
  files?: TransferTorrentFile[];
};

export type TransferMatch = {
  torrentId: string;
  status: TransferMatchStatus;
  /** Output folder to use (matched / ambiguous pick). */
  path?: string;
  /** Folder or file that was matched. */
  entryPath?: string;
  /** 0–1 confidence. */
  confidence: number;
  /** Other close candidates when ambiguous. */
  alternates?: { path: string; entryPath: string; confidence: number }[];
  reason?: string;
};

const normalize = (s: string): string =>
  stripPartialSuffix(s)
    .toLowerCase()
    .replace(/\.[a-f0-9]{8}$/i, "")
    .replace(/[\[\](){}_.-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();

/** Dice coefficient on character bigrams — cheap fuzzy similarity. */
export function fuzzyScore(a: string, b: string): number {
  const na = normalize(a);
  const nb = normalize(b);
  if (!na || !nb) return 0;
  if (na === nb) return 1;
  if (na.includes(nb) || nb.includes(na)) {
    const shorter = Math.min(na.length, nb.length);
    const longer = Math.max(na.length, nb.length);
    return 0.85 + 0.15 * (shorter / longer);
  }
  const bigrams = (s: string): Map<string, number> => {
    const m = new Map<string, number>();
    for (let i = 0; i < s.length - 1; i++) {
      const g = s.slice(i, i + 2);
      m.set(g, (m.get(g) ?? 0) + 1);
    }
    return m;
  };
  const A = bigrams(na);
  const B = bigrams(nb);
  let overlap = 0;
  for (const [g, c] of A) {
    const bc = B.get(g);
    if (bc) overlap += Math.min(c, bc);
  }
  const total = Math.max(
    1,
    [...A.values()].reduce((x, y) => x + y, 0) +
      [...B.values()].reduce((x, y) => x + y, 0),
  );
  return (2 * overlap) / total;
}

type Scored = { score: number; reason: string } | null;

/** Size check for an existing file vs what the torrent expects. */
const sizeFits = (
  size: number | undefined,
  length: number,
  partial: boolean | undefined,
): "exact" | "partial" | "unknown" | "too_big" | "mismatch" => {
  if (size === undefined) return "unknown";
  if (size === length) return "exact";
  if (size > length) return "too_big";
  // Smaller: fine for an in-progress file (qBit may not preallocate).
  return partial ? "partial" : "mismatch";
};

/** Score a torrent (with metadata) against one candidate. */
function scoreWithMetadata(
  t: TransferTorrentHint & { files: TransferTorrentFile[] },
  c: TransferCandidate,
): Scored {
  const files = t.files;
  const single = files.length === 1 && files[0].components.length === 1;

  if (single) {
    const f = files[0];
    const fname = f.components[0];
    if (c.kind === "file") {
      const exactName = c.name === fname;
      const fit = sizeFits(c.size, f.length, c.partial);
      if (fit === "too_big") return null;
      if (exactName && (fit === "exact" || fit === "partial"))
        return {
          score: 1,
          reason: fit === "exact" ? "same name and size" : "partial (.!qB)",
        };
      if (exactName)
        // Smaller but not marked partial: could be different data; writing
        // missing pieces would overwrite it. Show it, don't auto-match.
        return {
          score: 0.4,
          reason: "same name, smaller and not a .!qB partial",
        };
      if (fit !== "exact") return null;
      const nameScore = fuzzyScore(fname, c.name);
      if (nameScore < 0.6) return null;
      return {
        score: 0.5 + 0.4 * nameScore,
        reason: "same size, similar name",
      };
    }
    // A subfolder holding the single file (qBit "always create subfolder").
    // Not for the selected folders themselves: their files are candidates.
    if (c.isRoot) return null;
    const child = c.children?.find((ch) => !ch.isDir && ch.name === fname);
    if (child) {
      const fit = sizeFits(child.size, f.length, child.partial);
      if (fit === "exact" || fit === "partial")
        return { score: 0.95, reason: "folder contains the file" };
    }
    return null;
  }

  // Multi-file torrent: data lives in a folder whose direct children are the
  // torrent's top-level entries.
  if (c.kind !== "dir") return null;
  const tops = new Map<string, { isDir: boolean; length?: number }>();
  for (const f of files) {
    const top = f.components[0];
    if (f.components.length === 1)
      tops.set(top, { isDir: false, length: f.length });
    else if (!tops.has(top)) tops.set(top, { isDir: true });
  }
  const children = new Map((c.children ?? []).map((ch) => [ch.name, ch]));
  let hits = 0;
  let conflicts = 0;
  for (const [name, info] of tops) {
    const ch = children.get(name);
    if (!ch || ch.isDir !== info.isDir) continue;
    if (!info.isDir && info.length !== undefined) {
      const fit = sizeFits(ch.size, info.length, ch.partial);
      if (fit === "too_big") {
        conflicts++;
        continue;
      }
      // Smaller and not a .!qB partial: don't count it as evidence.
      if (fit === "mismatch") continue;
    }
    hits++;
  }
  if (conflicts > 0) return null;
  const overlap = tops.size ? hits / tops.size : 0;
  const nameScore = fuzzyScore(t.name ?? t.label, c.name);
  const exactName = !!t.name && t.name === c.name;
  if (overlap === 0) {
    // Nothing on disk matches; a same-named empty folder isn't a match.
    return {
      score: Math.min(0.4, nameScore * 0.4),
      reason: "no matching files",
    };
  }
  const score = Math.min(
    1,
    0.6 * overlap + 0.3 * nameScore + (exactName ? 0.1 : 0),
  );
  return {
    score,
    reason: `${hits}/${tops.size} entries present${exactName ? ", same name" : ""}`,
  };
}

/** Label-only fallback (no metadata, e.g. magnets). */
function scoreByLabel(t: TransferTorrentHint, c: TransferCandidate): Scored {
  const nameScore = fuzzyScore(t.name ?? t.label, c.name);
  return { score: nameScore * 0.7, reason: "name only (no metadata)" };
}

/**
 * Match each torrent hint to the best candidate folder/file.
 * With metadata: single-file torrents match files by exact name and size
 * (`.!qB` partials may be smaller); multi-file torrents match folders whose
 * children contain the torrent's top-level entries. Anything larger than the
 * torrent expects never matches (it can't be the same data).
 * Ambiguous when the top two candidates are within 0.08 and both >= 0.55.
 */
export function matchTransferFolders(
  torrents: TransferTorrentHint[],
  candidates: TransferCandidate[],
): TransferMatch[] {
  if (candidates.length === 0) {
    return torrents.map((t) => ({
      torrentId: t.id,
      status: "unmatched" as const,
      confidence: 0,
      reason: "no folders selected",
    }));
  }

  type Pair = {
    tid: string;
    path: string;
    entryPath: string;
    score: number;
    reason: string;
    partial: boolean;
  };
  const pairs: Pair[] = [];
  for (const t of torrents) {
    for (const c of candidates) {
      const s =
        t.files && t.files.length > 0
          ? scoreWithMetadata({ ...t, files: t.files }, c)
          : scoreByLabel(t, c);
      if (!s) continue;
      pairs.push({
        tid: t.id,
        path: c.path,
        entryPath: c.entryPath,
        score: s.score,
        reason: s.reason,
        partial: !!c.partial,
      });
    }
  }
  pairs.sort((a, b) => b.score - a.score);

  const byTorrent = new Map<string, Pair[]>();
  for (const p of pairs) {
    const list = byTorrent.get(p.tid) ?? [];
    list.push(p);
    byTorrent.set(p.tid, list);
  }

  // Greedy one-to-one assignment on the matched entry (several single-file
  // torrents can share an output folder, but not the same file).
  const used = new Set<string>();
  const assigned = new Map<string, Pair>();
  for (const p of pairs) {
    if (assigned.has(p.tid) || used.has(p.entryPath)) continue;
    if (p.score < 0.45) continue;
    assigned.set(p.tid, p);
    used.add(p.entryPath);
  }

  const results: TransferMatch[] = [];
  for (const t of torrents) {
    const ranked = byTorrent.get(t.id) ?? [];
    const best = assigned.get(t.id);
    const top = ranked[0];

    if (!best) {
      results.push({
        torrentId: t.id,
        status: "unmatched",
        confidence: top?.score ?? 0,
        reason: !top
          ? "no candidate data found"
          : top.score < 0.45
            ? `best ${Math.round(top.score * 100)}% too low (${top.reason})`
            : `best candidate taken by another torrent`,
      });
      continue;
    }

    const second = ranked.find((r) => r.entryPath !== best.entryPath);
    const amb =
      !!second && second.score >= 0.55 && best.score - second.score < 0.08;

    if (amb) {
      results.push({
        torrentId: t.id,
        status: "ambiguous",
        path: best.path,
        entryPath: best.entryPath,
        confidence: best.score,
        alternates: ranked.slice(0, 3).map((r) => ({
          path: r.path,
          entryPath: r.entryPath,
          confidence: r.score,
        })),
        reason: "multiple similar candidates",
      });
    } else {
      results.push({
        torrentId: t.id,
        status: "matched",
        path: best.path,
        entryPath: best.entryPath,
        confidence: best.score,
        reason: best.reason,
      });
    }
  }

  return results;
}

/** Prefer magnet dn=, else truncate. */
export function displayNameForSource(
  text: string,
  fallbackLabel?: string,
): string {
  const m = /[?&]dn=([^&]+)/i.exec(text);
  if (m) {
    try {
      return decodeURIComponent(m[1].replace(/\+/g, " "));
    } catch {
      return m[1];
    }
  }
  if (fallbackLabel) return fallbackLabel;
  return text.length > 72 ? text.slice(0, 69) + "…" : text;
}
