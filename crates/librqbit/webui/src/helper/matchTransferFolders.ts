/** Fuzzy-match download folders from another client onto torrents being added. */

export type TransferMatchStatus = "matched" | "unmatched" | "ambiguous";

export type TransferCandidate = {
  /** Absolute server path to a folder. */
  path: string;
  /** Basename of the folder. */
  name: string;
  /** Child basenames (files/dirs) when known — used for overlap scoring. */
  children?: string[];
};

export type TransferTorrentHint = {
  id: string;
  /** Display / torrent name if known. */
  label: string;
  /** Optional file basenames from .torrent metadata (when available). */
  files?: string[];
};

export type TransferMatch = {
  torrentId: string;
  status: TransferMatchStatus;
  /** Best matched folder path (matched / ambiguous pick). */
  path?: string;
  /** 0–1 confidence. */
  confidence: number;
  /** Other close candidates when ambiguous. */
  alternates?: { path: string; confidence: number }[];
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

function fileOverlap(
  torrentFiles: string[] | undefined,
  children: string[] | undefined,
): number {
  if (!torrentFiles?.length || !children?.length) return 0;
  const childSet = new Set(children.map(normalize));
  let hits = 0;
  for (const f of torrentFiles) {
    const base = normalize(f.split(/[/\\]/).pop() || f);
    if (childSet.has(base)) {
      hits++;
      continue;
    }
    for (const c of childSet) {
      if (fuzzyScore(base, c) >= 0.9) {
        hits++;
        break;
      }
    }
  }
  return hits / torrentFiles.length;
}

/**
 * Match each torrent hint to the best candidate folder.
 * Prefer high name similarity; boost with file-list overlap when present.
 * Ambiguous when top two scores are within 0.08 and both >= 0.55.
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

  type Pair = { tid: string; path: string; score: number };
  const pairs: Pair[] = [];
  for (const t of torrents) {
    for (const c of candidates) {
      const nameScore = Math.max(
        fuzzyScore(t.label, c.name),
        fuzzyScore(t.label, c.path.split(/[/\\]/).pop() || c.name),
      );
      const overlap = fileOverlap(t.files, c.children);
      const score = Math.min(
        1,
        nameScore * 0.7 + overlap * 0.45 + (overlap > 0.5 ? 0.1 : 0),
      );
      pairs.push({ tid: t.id, path: c.path, score });
    }
  }
  pairs.sort((a, b) => b.score - a.score);

  const byTorrent = new Map<string, Pair[]>();
  for (const p of pairs) {
    const list = byTorrent.get(p.tid) ?? [];
    list.push(p);
    byTorrent.set(p.tid, list);
  }

  const used = new Set<string>();
  const assigned = new Map<string, Pair>();
  for (const p of pairs) {
    if (assigned.has(p.tid) || used.has(p.path)) continue;
    if (p.score < 0.45) continue;
    assigned.set(p.tid, p);
    used.add(p.path);
  }

  const results: TransferMatch[] = [];
  for (const t of torrents) {
    const ranked = (byTorrent.get(t.id) ?? [])
      .slice()
      .sort((a, b) => b.score - a.score);
    const best = assigned.get(t.id);
    const top = ranked[0];
    const second = ranked[1];

    if (!best || !top || top.score < 0.45) {
      results.push({
        torrentId: t.id,
        status: "unmatched",
        confidence: top?.score ?? 0,
        reason: top
          ? `best ${Math.round((top.score || 0) * 100)}% too low`
          : "no candidates",
      });
      continue;
    }

    const amb =
      !!second &&
      second.path !== best.path &&
      second.score >= 0.55 &&
      best.score - second.score < 0.08;

    if (amb) {
      results.push({
        torrentId: t.id,
        status: "ambiguous",
        path: best.path,
        confidence: best.score,
        alternates: ranked.slice(0, 3).map((r) => ({
          path: r.path,
          confidence: r.score,
        })),
        reason: "multiple similar folders",
      });
    } else {
      results.push({
        torrentId: t.id,
        status: "matched",
        path: best.path,
        confidence: best.score,
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
