import { JSX, useContext, useEffect, useState } from "react";
import { ErrorDetails as ApiErrorDetails, TorrentListItem } from "./api-types";
import { TorrentFeed, browserInflater } from "./helper/torrentFeed";
import { APIContext } from "./context";
import { RootContent } from "./components/RootContent";
import { customSetInterval } from "./helper/customSetInterval";
import { LogStreamModal } from "./components/modal/LogStreamModal";
import { Header } from "./components/Header";
import { useTorrentStore } from "./stores/torrentStore";
import { useErrorStore } from "./stores/errorStore";
import { AlertModal } from "./components/modal/AlertModal";
import { useStatsStore } from "./stores/statsStore";
import { useUIStore } from "./stores/uiStore";
import { usePrefsStore } from "./stores/prefsStore";
import { Footer } from "./components/Footer";
import { SettingsButtons } from "./components/SettingsButtons";

export interface ErrorWithLabel {
  text: string;
  details?: ApiErrorDetails;
}

export interface ContextType {
  setCloseableError: (error: ErrorWithLabel | null) => void;
  refreshTorrents: () => void;
}

export const RqbitWebUI = (props: {
  title: string;
  version: string;
  menuButtons?: JSX.Element[];
}) => {
  let [logsOpened, setLogsOpened] = useState<boolean>(false);
  const setOtherError = useErrorStore((state) => state.setOtherError);

  const API = useContext(APIContext);

  const setTorrents = useTorrentStore((state) => state.setTorrents);
  const setTorrentsLoading = useTorrentStore(
    (state) => state.setTorrentsLoading,
  );
  const setRefreshTorrents = useTorrentStore(
    (state) => state.setRefreshTorrents,
  );

  /** Stores a fresh list; returns the update interval it calls for. */
  const applyTorrents = (torrents: TorrentListItem[]): number => {
    setTorrents(torrents);
    // Keep the selection across polls; drop torrents that went away.
    useUIStore.getState().pruneSelection(new Set(torrents.map((t) => t.id)));
    setOtherError(null);

    // Fast updates (1s) if any torrent is live/initializing, slow (5s) otherwise
    const hasActiveTorrents = torrents.some(
      (t) => t.stats?.state === "live" || t.stats?.state === "initializing",
    );
    return hasActiveTorrents ? 1000 : 5000;
  };

  /** Plain polling, for API implementations without the list stream. */
  const refreshTorrents = async (): Promise<number> => {
    setTorrentsLoading(true);
    try {
      const response = await API.listTorrents({ withStats: true });
      return applyTorrents(response.torrents);
    } catch (e) {
      setOtherError({ text: "Error refreshing torrents", details: e as any });
      console.error(e);
      return 5000;
    } finally {
      setTorrentsLoading(false);
    }
  };

  const setStats = useStatsStore((state) => state.setStats);

  // Keep the torrent list current: the server's list stream (WebSocket with deltas,
  // falling back to delta polling) where available, else plain polling.
  useEffect(() => {
    const wsUrl = API.getTorrentListStreamUrl?.() ?? null;
    const poll = API.pollTorrentList;
    if (!poll) {
      setRefreshTorrents(refreshTorrents as unknown as () => void);
      return customSetInterval(async () => refreshTorrents(), 0);
    }
    setTorrentsLoading(true);
    const feed: TorrentFeed = new TorrentFeed(
      {
        wsUrl,
        poll,
        WebSocket: typeof WebSocket === "function" ? (WebSocket as any) : null,
        makeInflater: browserInflater(),
        setTimeout: (f, ms) => window.setTimeout(f, ms),
        clearTimeout: (t) => window.clearTimeout(t as number),
        now: () => Date.now(),
      },
      {
        onTorrents: (torrents) => {
          setTorrentsLoading(false);
          feed.setTickMs(applyTorrents(torrents));
        },
        onError: (e) => {
          if (e === null) return;
          setTorrentsLoading(false);
          setOtherError({
            text: "Error refreshing torrents",
            details: e as any,
          });
          console.error(e);
        },
      },
    );
    setRefreshTorrents(() => feed.refresh());
    feed.start();
    return () => feed.stop();
  }, []);

  // Preferences the UI itself uses (remove/delete behaviour); refreshed when
  // the window regains focus in case another client changed them.
  useEffect(() => {
    const load = () => usePrefsStore.getState().loadPreferences(API);
    load();
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, [API]);

  useEffect(() => {
    return customSetInterval(
      async () =>
        API.stats().then(
          (stats) => {
            setStats(stats);
            return 1000;
          },
          (e) => {
            console.error(e);
            return 5000;
          },
        ),
      0,
    );
  }, []);

  return (
    <div className="bg-surface h-dvh flex flex-col overflow-hidden">
      <Header
        title={props.title}
        version={props.version}
        settingsSlot={
          <SettingsButtons
            onLogsClick={() => setLogsOpened(true)}
            menuButtons={props.menuButtons}
          />
        }
      />

      <div className="grow min-h-0">
        <RootContent />
      </div>

      <Footer />

      <LogStreamModal show={logsOpened} onClose={() => setLogsOpened(false)} />
      <AlertModal />
    </div>
  );
};
