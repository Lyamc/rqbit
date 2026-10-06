import { RemovePolicy, SessionPreferences } from "../api-types";

export const KEEP_POLICY: RemovePolicy = { complete: "keep", incomplete: "keep" };

/** The saved remove policy (migrating the legacy single action). */
export function policyFromPrefs(
  prefs: Pick<
    SessionPreferences,
    "remove_policy" | "default_remove_action"
  > | null,
): RemovePolicy {
  if (!prefs) return KEEP_POLICY;
  if (prefs.remove_policy) return prefs.remove_policy;
  if (prefs.default_remove_action === "delete_files") {
    return { complete: "delete", incomplete: "delete" };
  }
  return KEEP_POLICY;
}

export interface RemoveGroups {
  complete: number;
  incomplete: number;
}

/** Whether removing these groups with this policy touches files on disk. */
export function policyDeletesFiles(
  policy: RemovePolicy,
  groups?: RemoveGroups,
): boolean {
  const anyComplete = groups ? groups.complete > 0 : true;
  const anyIncomplete = groups ? groups.incomplete > 0 : true;
  return (
    (anyComplete && policy.complete === "delete") ||
    (anyIncomplete && policy.incomplete !== "keep")
  );
}

export interface RemovePlan {
  /** Show the confirmation dialog. */
  confirm: boolean;
  /** Preset (or, without a dialog, the policy used). */
  policy: RemovePolicy;
}

/**
 * How Remove behaves given the server preferences (`confirm_remove`,
 * `remove_policy`) and, once known, how many selected torrents are complete
 * or incomplete. Anything that deletes files always shows the dialog, and
 * unknown preferences (not loaded yet) also confirm.
 */
export function planRemove(
  prefs: Pick<
    SessionPreferences,
    "confirm_remove" | "default_remove_action" | "remove_policy"
  > | null,
  groups?: RemoveGroups,
): RemovePlan {
  const policy = policyFromPrefs(prefs);
  if (!prefs) return { confirm: true, policy };
  const confirm =
    prefs.confirm_remove !== false || policyDeletesFiles(policy, groups);
  return { confirm, policy };
}

export function completeActionText(action: RemovePolicy["complete"]): string {
  return action === "delete"
    ? "remove and delete their files"
    : "remove, keep files on disk";
}

export function incompleteActionText(
  action: RemovePolicy["incomplete"],
  nothingDone = 0,
  total = 0,
  /** "Move files individually as they complete": finished files already moved. */
  filesMoveIndividually = false,
): string {
  switch (action) {
    case "keep":
      return "remove, keep partial files on disk";
    case "delete":
      return "remove and delete all their files (including finished ones)";
    case "finish": {
      let s = filesMoveIndividually
        ? "remove and delete the unfinished files; finished files are kept (they have already moved)"
        : "finish what's done: delete unfinished files, run completion actions on the finished ones, then remove";
      if (nothingDone > 0) {
        s +=
          nothingDone === total
            ? ". Nothing is complete yet, so all partial data is deleted and the torrent removed"
            : `. ${nothingDone} of them have no complete file yet: all their partial data is deleted`;
      }
      return s;
    }
  }
}
