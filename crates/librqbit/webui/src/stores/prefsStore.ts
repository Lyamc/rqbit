import { create } from "zustand";
import { RqbitAPI, SessionPreferences } from "../api-types";

/** Server preferences the UI itself needs (remove/delete behaviour). */
export interface PrefsStore {
  preferences: SessionPreferences | null;
  setPreferences: (p: SessionPreferences) => void;
  /** Loads GET /torrents/preferences; errors leave the previous value. */
  loadPreferences: (api: RqbitAPI) => Promise<void>;
}

export const usePrefsStore = create<PrefsStore>((set) => ({
  preferences: null,
  setPreferences: (preferences) => set({ preferences }),
  loadPreferences: async (api) => {
    try {
      set({ preferences: await api.getPreferences() });
    } catch (e) {
      console.warn("could not load preferences", e);
    }
  },
}));
