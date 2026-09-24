/** Extract magnet links (and optional torrent HTTP URLs) from messy paste text. */
const MAGNET_RE = /magnet:\?[^\s<>"'\]]+/gi;
const TORRENT_URL_RE = /https?:\/\/[^\s<>"']+\.torrent(?:\?[^\s<>"']*)?/gi;

export function extractMagnetLinks(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];

  const push = (raw: string) => {
    // Trim trailing punctuation commonly left from copy/paste.
    const cleaned = raw.replace(/[.,;:)\]}>]+$/g, "");
    if (!cleaned || seen.has(cleaned)) return;
    seen.add(cleaned);
    out.push(cleaned);
  };

  for (const m of text.matchAll(MAGNET_RE)) {
    push(m[0]);
  }
  for (const m of text.matchAll(TORRENT_URL_RE)) {
    push(m[0]);
  }

  return out;
}
