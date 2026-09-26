import { create } from "zustand";
import {
  TorrentSortColumn,
  SortDirection,
  StatusFilter,
} from "../helper/torrentFilters";
import {
  Nav,
  SelectionState,
  clearSelected,
  clickSelect,
  navigate,
  pruneSelection,
  rangeSelect,
  selectAllIds,
  toggleFocused,
  toggleSelect,
} from "../helper/selection";

const LARGE_SCREEN_BREAKPOINT = 1024;

function getDefaultViewMode(): "full" | "compact" {
  return window.innerWidth >= LARGE_SCREEN_BREAKPOINT ? "compact" : "full";
}

export interface UIStore {
  viewMode: "full" | "compact";
  setViewMode: (mode: "full" | "compact") => void;
  toggleViewMode: () => void;

  searchQuery: string;
  setSearchQuery: (query: string) => void;

  statusFilter: StatusFilter;
  setStatusFilter: (filter: StatusFilter) => void;

  selectedTorrentIds: Set<number>;
  /** Range anchor. */
  lastSelectedId: number | null;
  /** Keyboard cursor (focus ring). */
  focusedTorrentId: number | null;
  selectTorrent: (id: number) => void;
  toggleSelection: (id: number) => void;
  /** Shift+click; `additive` = ctrl+shift+click. */
  selectRange: (id: number, orderedIds: number[], additive?: boolean) => void;
  deselectTorrent: (id: number) => void;
  clearSelection: () => void;
  selectAll: (ids: number[]) => void;
  /** Arrows/Home/End; returns the new focus index in `orderedIds` (-1: none). */
  navigateSelection: (
    nav: Nav,
    extend: boolean,
    orderedIds: number[],
  ) => number;
  /** Space. */
  toggleFocusedSelection: (orderedIds: number[]) => void;
  /** Drop ids that no longer exist (after polling). */
  pruneSelection: (existingIds: Set<number>) => void;

  detailsModalTorrentId: number | null;
  openDetailsModal: (id: number) => void;
  closeDetailsModal: () => void;
}

const toSel = (s: UIStore): SelectionState => ({
  selected: s.selectedTorrentIds,
  anchor: s.lastSelectedId,
  focus: s.focusedTorrentId,
});

const fromSel = (s: SelectionState) => ({
  selectedTorrentIds: s.selected,
  lastSelectedId: s.anchor,
  focusedTorrentId: s.focus,
});

export const useUIStore = create<UIStore>((set, get) => ({
  viewMode: getDefaultViewMode(),

  setViewMode: (mode) => {
    set({ viewMode: mode });
  },

  toggleViewMode: () => {
    const newMode = get().viewMode === "compact" ? "full" : "compact";
    set({ viewMode: newMode });
  },

  searchQuery: "",
  setSearchQuery: (query) => set({ searchQuery: query }),

  statusFilter: "all",
  setStatusFilter: (filter) => {
    set({ statusFilter: filter });
  },

  selectedTorrentIds: new Set<number>(),
  lastSelectedId: null,
  focusedTorrentId: null,

  selectTorrent: (id) => set(fromSel(clickSelect(toSel(get()), id))),

  toggleSelection: (id) => set(fromSel(toggleSelect(toSel(get()), id))),

  selectRange: (id, orderedIds, additive = false) =>
    set(fromSel(rangeSelect(toSel(get()), id, orderedIds, additive))),

  deselectTorrent: (id) => {
    const current = get().selectedTorrentIds;
    if (current.has(id)) {
      const next = new Set(current);
      next.delete(id);
      set({ selectedTorrentIds: next });
    }
  },

  clearSelection: () => set(fromSel(clearSelected(toSel(get())))),

  selectAll: (ids) => set(fromSel(selectAllIds(toSel(get()), ids))),

  navigateSelection: (nav, extend, orderedIds) => {
    const { state, index } = navigate(toSel(get()), nav, extend, orderedIds);
    set(fromSel(state));
    return index;
  },

  toggleFocusedSelection: (orderedIds) =>
    set(fromSel(toggleFocused(toSel(get()), orderedIds))),

  pruneSelection: (existingIds) => {
    const cur = toSel(get());
    const next = pruneSelection(cur, (id) => existingIds.has(id));
    if (next !== cur) set(fromSel(next));
  },

  detailsModalTorrentId: null,
  openDetailsModal: (id) =>
    set({
      detailsModalTorrentId: id,
      selectedTorrentIds: new Set([id]),
      lastSelectedId: id,
      focusedTorrentId: id,
    }),
  closeDetailsModal: () => set({ detailsModalTorrentId: null }),
}));
