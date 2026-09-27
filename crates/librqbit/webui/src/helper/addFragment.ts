/**
 * `#add=<link>` support: the web UI registers itself as the browser's
 * magnet: handler (navigator.registerProtocolHandler) with the target
 * `<this page>#add=%s`, so clicking a magnet link opens the Add window here
 * with the link staged (nothing is added until the user clicks Add).
 */

/** The magnet / http(s) link in a `#add=` fragment, or null. */
export function parseAddFragment(hash: string): string | null {
  const frag = hash.replace(/^#/, "");
  if (!frag) return null;
  let v: string | null;
  try {
    v = new URLSearchParams(frag).get("add");
  } catch {
    return null;
  }
  const t = (v ?? "").trim();
  if (/^magnet:/i.test(t) || /^https?:\/\//i.test(t)) return t;
  return null;
}

/** registerProtocolHandler target for a page location. */
export function magnetHandlerUrl(loc: { origin: string; pathname: string }): string {
  return `${loc.origin}${loc.pathname}#add=%s`;
}

let consumed = false;

/**
 * The page's `#add=` link, once per page load (and again after a later
 * hashchange). Clears the fragment so reloads don't stage it again.
 */
export function takeAddFromLocation(fromHashChange = false): string | null {
  if (consumed && !fromHashChange) return null;
  const link = parseAddFragment(window.location.hash);
  if (!link) return null;
  consumed = true;
  try {
    window.history.replaceState(
      null,
      "",
      window.location.pathname + window.location.search,
    );
  } catch {
    // ignore
  }
  return link;
}

/** Ask the browser to open magnet links with this page. */
export function registerMagnetHandler(): { ok: boolean; message: string } {
  const nav = navigator as Navigator & {
    registerProtocolHandler?: (scheme: string, url: string) => void;
  };
  if (typeof nav.registerProtocolHandler !== "function") {
    return {
      ok: false,
      message: "This browser doesn't support registering protocol handlers.",
    };
  }
  try {
    nav.registerProtocolHandler("magnet", magnetHandlerUrl(window.location));
    return {
      ok: true,
      message:
        "Asked the browser to open magnet links here. Confirm in its prompt if it shows one (Chrome: the icon at the right of the address bar).",
    };
  } catch (e) {
    return {
      ok: false,
      message: `The browser refused: ${e instanceof Error ? e.message : String(e)}`,
    };
  }
}
