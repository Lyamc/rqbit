import {
  DownloadOrderPatch,
  DownloadOrderView,
  FileOrder,
} from "../api-types";
import type { MenuItem } from "../components/ContextMenu";

export const FILE_ORDER_NAMES: Record<FileOrder, string> = {
  name: "By name",
  torrent: "Torrent order",
  smallest_first: "Smallest first",
  largest_first: "Largest first",
};

type Tri = boolean | null | undefined;

/** On / Off / Default submenu for a tri-state override. */
function triMenu(
  current: Tri,
  known: boolean,
  effective: boolean | undefined,
  defaultLabel: string,
  apply: (v: boolean | null) => void,
): MenuItem[] {
  return [
    { label: "On", checked: known && current === true, onClick: () => apply(true) },
    { label: "Off", checked: known && current === false, onClick: () => apply(false) },
    {
      label: defaultLabel,
      hint: known && effective !== undefined ? (effective ? "on" : "off") : undefined,
      checked: known && current == null,
      onClick: () => apply(null),
    },
  ];
}

/**
 * Torrent-wide download order submenu. `view` (single selection) marks the
 * current settings; with several torrents nothing is checked and a choice
 * applies to all of them.
 */
export function torrentOrderMenu(
  view: DownloadOrderView | null,
  apply: (patch: DownloadOrderPatch) => void,
): MenuItem[] {
  const known = !!view;
  const t = view?.torrent;
  const g = view?.global;
  return [
    {
      label: "Sequential file download",
      submenu: triMenu(t?.sequential_files, known, g?.sequential_files, "Use default", (v) =>
        apply({ sequential_files: v }),
      ),
    },
    {
      label: "File order",
      submenu: [
        ...(Object.keys(FILE_ORDER_NAMES) as FileOrder[]).map((k) => ({
          label: FILE_ORDER_NAMES[k],
          checked: known && t?.file_order === k,
          onClick: () => apply({ file_order: k }),
        })),
        {
          label: "Use default",
          hint: g ? FILE_ORDER_NAMES[g.file_order].toLowerCase() : undefined,
          checked: known && t?.file_order == null,
          onClick: () => apply({ file_order: null }),
        },
      ],
    },
    {
      label: "Sequential download (all files)",
      submenu: triMenu(t?.sequential, known, g?.sequential, "Use default", (v) =>
        apply({ sequential: v }),
      ),
    },
    {
      label: "First and last pieces first (all files)",
      submenu: triMenu(t?.first_last_first, known, g?.first_last_first, "Use default", (v) =>
        apply({ first_last_first: v }),
      ),
    },
    { separator: true },
    { label: "Reset to defaults", onClick: () => apply({ reset: true }) },
  ];
}

/** Per-file submenu items for the given file ids. */
export function fileOrderMenu(
  view: DownloadOrderView | null,
  ids: number[],
  apply: (patch: DownloadOrderPatch) => void,
): MenuItem[] {
  const files = view ? view.files.filter((f) => ids.includes(f.id)) : [];
  const same = <T,>(xs: T[]): T | undefined =>
    xs.length > 0 && xs.every((x) => x === xs[0]) ? xs[0] : undefined;
  const known = files.length === ids.length && ids.length > 0;
  const seq = same(files.map((f) => f.override.sequential ?? null));
  const fl = same(files.map((f) => f.override.first_last_first ?? null));
  const knownSeq = known && seq !== undefined;
  const knownFl = known && fl !== undefined;
  const torrentSeq = view?.effective.sequential;
  const torrentFl = view?.effective.first_last_first;
  return [
    {
      label: "Sequential download",
      submenu: triMenu(seq, knownSeq, torrentSeq, "Same as torrent", (v) =>
        apply({ files: [{ ids, sequential: v }] }),
      ),
    },
    {
      label: "Download first and last pieces first",
      submenu: triMenu(fl, knownFl, torrentFl, "Same as torrent", (v) =>
        apply({ files: [{ ids, first_last_first: v }] }),
      ),
    },
  ];
}
