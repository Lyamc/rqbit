import { AddJobStage, AddTorrentResponse } from "../api-types";

/** How an add that the server answered 2xx ended up. "resolving" is a success: the
 *  magnet is in rqbit (listed as "Resolving metadata") and keeps fetching its
 *  metadata in the background for as long as it takes; it never fails for lack of
 *  peers. */
export type AddOutcome = "added" | "resolving" | "already";

export function addOutcome(
  res: Pick<AddTorrentResponse, "resolving" | "already_managed" | "state"> | undefined,
  finalStage?: AddJobStage,
): AddOutcome {
  if (res?.already_managed || finalStage === "already_managed") return "already";
  if (
    res?.resolving ||
    res?.state === "resolving_metadata" ||
    finalStage === "resolving_in_background"
  ) {
    return "resolving";
  }
  return "added";
}

export const RESOLVING_NOTE =
  "Added — resolving metadata in the background. It waits for peers as long as it takes (never fails for lack of peers); see the torrent list.";

/** Staging-queue label for a finished ("ok") item. */
export function okLabel(outcome: AddOutcome | undefined): string {
  switch (outcome) {
    case "resolving":
      return "added · resolving metadata";
    case "already":
      return "already in rqbit";
    default:
      return "added";
  }
}
