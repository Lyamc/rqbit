/** Shared compact torrent table column layout (CSS grid).
 * Header and rows must use TORRENT_TABLE_GRID so columns cannot drift.
 * Fixed tracks match prior table-fixed widths (w-8/w-12/w-20/w-24/w-16).
 */
export const TORRENT_TABLE_GRID =
  "grid w-full items-center " +
  "grid-cols-[2rem_2rem_3rem_2.5rem_minmax(0,1fr)_10rem_5rem_6rem_5rem_5rem_5rem_5rem_5rem_4rem]";

/** Cell padding shared by header labels and row cells */
export const TORRENT_TABLE_CELL_PAD = "px-2";
