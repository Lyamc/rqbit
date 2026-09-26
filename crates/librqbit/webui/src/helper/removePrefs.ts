import { SessionPreferences } from "../api-types";

export interface RemovePlan {
  /** Show the confirmation dialog. */
  confirm: boolean;
  /** Preset (or, without a dialog, the action) of "also delete files". */
  deleteFiles: boolean;
}

/**
 * How Remove/Delete behaves given the server preferences (`confirm_remove`,
 * `default_remove_action`). Deleting files never happens without a
 * confirmation, so a "delete files" default always shows the dialog.
 * Unknown preferences (not loaded yet) also confirm.
 */
export function planRemove(
  prefs: Pick<
    SessionPreferences,
    "confirm_remove" | "default_remove_action"
  > | null,
): RemovePlan {
  if (!prefs) return { confirm: true, deleteFiles: false };
  const deleteFiles = prefs.default_remove_action === "delete_files";
  const confirm = prefs.confirm_remove !== false || deleteFiles;
  return { confirm, deleteFiles };
}
