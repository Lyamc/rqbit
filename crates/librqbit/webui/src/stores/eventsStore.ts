import { create } from "zustand";
import { EventSummary, RqbitAPI } from "../api-types";

const LAST_SEEN_KEY = "rqbit-events-last-seen-seq";

function loadLastSeen(): number | null {
  try {
    const v = window.localStorage.getItem(LAST_SEEN_KEY);
    if (v === null) return null;
    const n = parseInt(v, 10);
    return isNaN(n) ? null : n;
  } catch {
    return null;
  }
}

export interface EventsStore {
  open: boolean;
  /** Filter preset when opened from a torrent's details. */
  presetInfoHash: string | null;
  openEvents: (infoHash?: string | null) => void;
  closeEvents: () => void;

  /** Highest event seq the user has seen (localStorage). */
  lastSeenSeq: number | null;
  markSeen: (seq: number) => void;

  summary: EventSummary | null;
  refreshSummary: (api: RqbitAPI) => Promise<void>;
}

export const useEventsStore = create<EventsStore>((set, get) => ({
  open: false,
  presetInfoHash: null,
  openEvents: (infoHash) =>
    set({ open: true, presetInfoHash: infoHash ?? null }),
  closeEvents: () => set({ open: false, presetInfoHash: null }),

  lastSeenSeq: loadLastSeen(),
  markSeen: (seq) => {
    try {
      window.localStorage.setItem(LAST_SEEN_KEY, String(seq));
    } catch {}
    set({ lastSeenSeq: seq });
  },

  summary: null,
  refreshSummary: async (api) => {
    if (!api.getEventsSummary) return;
    let last = get().lastSeenSeq;
    try {
      // First run: nothing is "unseen" yet except what happens from now on.
      if (last === null) {
        const s = await api.getEventsSummary();
        get().markSeen(s.latest_seq);
        last = s.latest_seq;
      }
      const s = await api.getEventsSummary(last);
      set({ summary: s });
    } catch {
      // Older server without the events API: keep quiet.
    }
  },
}));
