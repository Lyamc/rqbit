/** Extract magnets and http(s) .torrent URLs from messy paste text. */
const MAGNET_RE = /magnet:\?[^\s<>"'\]]+/gi;
const TORRENT_URL_RE = /https?:\/\/[^\s<>"']+\.torrent(?:\?[^\s<>"']*)?/gi;
export type ParsedSource = {
  value: string;
  kind: "magnet" | "url";
};

function cleanToken(raw: string): string {
  return raw.replace(/[.,;:)\]}>]+$/g, "");
}

/** Magnets + explicit *.torrent URLs (preferred detector). */
export function extractTorrentSources(text: string): ParsedSource[] {
  const seen = new Set<string>();
  const out: ParsedSource[] = [];

  const push = (raw: string, kind: "magnet" | "url") => {
    const cleaned = cleanToken(raw);
    if (!cleaned || seen.has(cleaned)) return;
    seen.add(cleaned);
    out.push({ value: cleaned, kind });
  };

  for (const m of text.matchAll(MAGNET_RE)) {
    push(m[0], "magnet");
  }
  for (const m of text.matchAll(TORRENT_URL_RE)) {
    push(m[0], "url");
  }

  // If nothing matched, also accept line-split http(s) URLs (non-.torrent paths
  // may still resolve to torrent files server-side).
  if (out.length === 0) {
    for (const line of text.split(/\s+/)) {
      const t = cleanToken(line.trim());
      if (!t) continue;
      if (/^magnet:/i.test(t)) push(t, "magnet");
      else if (/^https?:\/\//i.test(t)) push(t, "url");
    }
  }

  return out;
}

/** Back-compat: return just the string values. */
export function extractMagnetLinks(text: string): string[] {
  return extractTorrentSources(text).map((s) => s.value);
}
