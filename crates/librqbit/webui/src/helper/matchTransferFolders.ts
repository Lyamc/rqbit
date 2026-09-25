/** Fuzzy-match download folders from another client onto torrents being added. */

export type TransferMatchStatus = "matched" | "unmatched" | "ambiguous";

export type TransferCandidateChild = {
  /** Basename as on disk. */
  name: string;
  isDir: boolean;
  size?: number;
};

export type ExpectedFileFit =
  /** Present under its final name with the expected size. */
  | { kind: "exact"; name: string }
  /** Missing, but a unique `<name><suffix>` sibling (another client's
   *  in-progress file: `.!qB`, `.part`, ...) that isn't larger. */
  | { kind: "partial"; name: string }
  /** Present under its final name but larger: different data. */
  | { kind: "too_big"; name: string }
  /** Present under its final name but smaller: not trusted as evidence. */
  | { kind: "smaller"; name: string }
  /** Several suffixed candidates: the server won't pick one. */
  | { kind: "ambiguous"; names: string[] }
  | { kind: "none" };

/**
 * Find a torrent file among a folder's entries, the same way the server's
 * adoption does (generic, no client-specific suffixes):
 * the exact name wins; otherwise exactly one sibling named
 * `<expected><non-empty suffix>` that is not itself an expected name (e.g.
 * split archives `a.zip.0.part` when the torrent has them), does not belong
 * to a longer expected name, and is not larger than expected.
 * `expectedNames`: all names the torrent expects in this folder.
 */
export function findExpectedFile(
  expected: string,
  length: number,
  entries: TransferCandidateChild[],
  expectedNames: Set<string>,
): ExpectedFileFit {
  const exact = entries.find((e) => e.name === expected);
  if (exact) {
    if (exact.isDir) return { kind: "none" };
    if (exact.size === undefined || exact.size === length)
      return { kind: "exact", name: exact.name };
    return exact.size > length
      ? { kind: "too_big", name: exact.name }
      : { kind: "smaller", name: exact.name };
  }
  const longer = [...expectedNames].filter(
    (n) => n.length > expected.length && n.startsWith(expected),
  );
  const cands = entries.filter(
    (e) =>
      !e.isDir &&
      e.name.length > expected.length &&
      e.name.startsWith(expected) &&
      !expectedNames.has(e.name) &&
      !longer.some((l) => e.name.startsWith(l)) &&
      (e.size === undefined || e.size <= length),
  );
  if (cands.length === 1) return { kind: "partial", name: cands[0].name };
  if (cands.length > 1)
    return { kind: "ambiguous", names: cands.map((c) => c.name) };
  return { kind: "none" };
}

export type TransferCandidate = {
  /** Server path of the folder or file this candidate represents. */
  entryPath: string;
  /** Output folder rqbit should use if matched: the folder itself for a
   *  directory, the containing folder for a single file. */
  path: string;
  /** Basename. */
  name: string;
  kind: "dir" | "file";
  /** Files only. */
  size?: number;
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
  s
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

type Scored = {
  score: number;
  reason: string;
  /** Matched entry, when more specific than the candidate itself. */
  entryPath?: string;
} | null;

const joinPath = (dir: string, name: string) =>
  `${dir.replace(/[/\\]+$/, "")}/${name}`;

/** Score a torrent (with metadata) against one candidate folder. */
function scoreWithMetadata(
  t: TransferTorrentHint & { files: TransferTorrentFile[] },
  c: TransferCandidate,
): Scored {
  if (c.kind !== "dir") return null;
  const files = t.files;
  const children = c.children ?? [];
  const single = files.length === 1 && files[0].components.length === 1;

  if (single) {
    // The folder holding the file is the output folder (a selected folder
    // itself, or a per-torrent subfolder).
    const f = files[0];
    const fname = f.components[0];
    const fit = findExpectedFile(fname, f.length, children, new Set([fname]));
    switch (fit.kind) {
      case "exact":
        return {
          score: 1,
          reason: "same name and size",
          entryPath: joinPath(c.path, fit.name),
        };
      case "partial":
        return {
          score: 0.97,
          reason: `partial (${fit.name})`,
          entryPath: joinPath(c.path, fit.name),
        };
      case "smaller":
        // Could be different data; writing missing pieces would overwrite
        // it. Show it, don't auto-match.
        return {
          score: 0.4,
          reason: "same name but smaller, and no partial-file suffix",
          entryPath: joinPath(c.path, fit.name),
        };
      case "ambiguous":
        return {
          score: 0.4,
          reason: `several partial files: ${fit.names.join(", ")}`,
          entryPath: joinPath(c.path, fname),
        };
      default:
        return null;
    }
  }

  // Multi-file torrent: data lives in a folder whose direct children are the
  // torrent's top-level entries.
  // Top-level entries with their total size (a directory's size is the sum
  // of the torrent files under it).
  const tops = new Map<string, { isDir: boolean; bytes: number }>();
  let totalBytes = 0;
  for (const f of files) {
    const top = f.components[0];
    const isDir = f.components.length > 1;
    const cur = tops.get(top) ?? { isDir, bytes: 0 };
    cur.bytes += f.length;
    tops.set(top, cur);
    totalBytes += f.length;
  }
  const topNames = new Set(tops.keys());
  let hits = 0;
  let hitBytes = 0;
  let partials = 0;
  let conflicts = 0;
  for (const [name, info] of tops) {
    if (info.isDir) {
      if (children.some((ch) => ch.isDir && ch.name === name)) {
        hits++;
        hitBytes += info.bytes;
      }
      continue;
    }
    const fit = findExpectedFile(name, info.bytes, children, topNames);
    if (fit.kind === "too_big") {
      conflicts++;
      continue;
    }
    // Smaller without a suffix / ambiguous: don't count it as evidence.
    if (fit.kind !== "exact" && fit.kind !== "partial") continue;
    if (fit.kind === "partial") partials++;
    hits++;
    hitBytes += info.bytes;
  }
  if (conflicts > 0) return null;
  const countOverlap = tops.size ? hits / tops.size : 0;
  // Weigh by size so shared boilerplate (nfo/jpg/txt that many releases
  // carry) doesn't make an unrelated folder look like a match.
  const byteOverlap = totalBytes > 0 ? hitBytes / totalBytes : countOverlap;
  const nameScore = fuzzyScore(t.name ?? t.label, c.name);
  const exactName = !!t.name && t.name === c.name;
  if (hits === 0) {
    // Nothing on disk matches; a same-named empty folder isn't a match.
    return {
      score: Math.min(0.4, nameScore * 0.4),
      reason: "no matching files",
    };
  }
  let score = Math.min(
    1,
    0.55 * byteOverlap +
      0.15 * countOverlap +
      0.3 * nameScore +
      (exactName ? 0.15 : 0),
  );
  // Without the exact torrent name, require most of the data to be there.
  if (!exactName && byteOverlap < 0.5) score = Math.min(score, 0.44);
  return {
    score,
    reason: `${hits}/${tops.size} entries (${Math.round(byteOverlap * 100)}% of size) present${partials ? `, ${partials} partial` : ""}${exactName ? ", same name" : ""}`,
  };
}

/** Label-only fallback (no metadata, e.g. magnets). */
function scoreByLabel(t: TransferTorrentHint, c: TransferCandidate): Scored {
  const nameScore = fuzzyScore(t.name ?? t.label, c.name);
  return { score: nameScore * 0.7, reason: "name only (no metadata)" };
}

/**
 * Match each torrent hint to the best candidate folder/file.
 * With metadata: single-file torrents match the folder holding the file
 * (exact name and size, or a unique `<name><suffix>` partial that isn't
 * larger — see findExpectedFile); multi-file torrents match folders whose
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
        entryPath: s.entryPath ?? c.entryPath,
        score: s.score,
        reason: s.reason,
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
