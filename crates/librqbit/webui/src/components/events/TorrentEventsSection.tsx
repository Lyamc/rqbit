import { useContext, useEffect, useState } from "react";
import { APIContext } from "../../context";
import { EventRecord, TorrentListItem } from "../../api-types";
import { useEventsStore } from "../../stores/eventsStore";
import { EventRow } from "./EventRow";

/** Repair count + recent events for one torrent (details panel). */
export const TorrentEventsSection: React.FC<{ torrent: TorrentListItem }> = ({
  torrent,
}) => {
  const api = useContext(APIContext);
  const openEvents = useEventsStore((s) => s.openEvents);
  const latestSeq = useEventsStore((s) => s.summary?.latest_seq);
  const [events, setEvents] = useState<EventRecord[] | null>(null);
  const repairCount = torrent.stats?.repair_count ?? 0;

  useEffect(() => {
    if (!api.getEvents) return;
    let cancelled = false;
    api
      .getEvents({ info_hash: torrent.info_hash, limit: 5 })
      .then((p) => !cancelled && setEvents(p.events))
      .catch(() => !cancelled && setEvents([]));
    return () => {
      cancelled = true;
    };
  }, [api, torrent.info_hash, latestSeq, repairCount]);

  if (!api.getEvents || events === null) return null;
  if (events.length === 0 && repairCount === 0) return null;

  return (
    <div className="flex flex-col gap-1" data-testid="torrent-events">
      <div className="flex items-center gap-3">
        <span className="text-tertiary">Repairs</span>
        <span data-testid="torrent-repair-count">{repairCount}</span>
        <span className="text-tertiary">· Recent events</span>
        <div className="flex-1" />
        <a
          href="#"
          className="text-primary hover:underline text-xs"
          onClick={(e) => {
            e.preventDefault();
            openEvents(torrent.info_hash);
          }}
        >
          All events
        </a>
      </div>
      {events.length > 0 && (
        <div className="border border-divider rounded divide-y divide-divider">
          {events.map((e) => (
            <EventRow key={e.seq} event={e} compact />
          ))}
        </div>
      )}
    </div>
  );
};
