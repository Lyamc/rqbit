import { useContext, useEffect } from "react";
import { BsActivity } from "react-icons/bs";
import { APIContext } from "../../context";
import { useEventsStore } from "../../stores/eventsStore";
import { IconButton } from "../buttons/IconButton";
import { EventsModal } from "./EventsModal";

/** Header button for the Events view, with a badge for unseen repairs/errors. */
export const EventsButton: React.FC = () => {
  const api = useContext(APIContext);
  const summary = useEventsStore((s) => s.summary);
  const refreshSummary = useEventsStore((s) => s.refreshSummary);
  const openEvents = useEventsStore((s) => s.openEvents);
  const lastSeenSeq = useEventsStore((s) => s.lastSeenSeq);

  useEffect(() => {
    if (!api.getEventsSummary) return;
    refreshSummary(api);
    const t = setInterval(() => refreshSummary(api), 15000);
    return () => clearInterval(t);
  }, [api, lastSeenSeq]);

  if (!api.getEventsSummary) return null;
  const unseen = summary?.unseen;
  const count = unseen ? unseen.repairs + unseen.errors : 0;
  const title = unseen
    ? `Events: ${unseen.repairs} new repair(s), ${unseen.errors} new error(s) since last viewed`
    : "Events";

  return (
    <>
      <IconButton onClick={() => openEvents()} title={title}>
        <span className="relative inline-flex" data-testid="events-button">
          <BsActivity />
          {count > 0 && (
            <span
              className={`absolute -top-2 -right-2.5 min-w-4 h-4 px-1 rounded-full text-[10px] leading-4 text-center text-white font-semibold ${unseen && unseen.errors > 0 ? "bg-error-bg" : "bg-warning-bg"}`}
              data-testid="events-badge"
            >
              {count > 99 ? "99+" : count}
            </span>
          )}
        </span>
      </IconButton>
      <EventsModal />
    </>
  );
};
