import React from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormCheckbox } from "../forms/FormCheckbox";
import {
  CompleteRemoveAction,
  IncompleteRemoveAction,
  SessionPreferences,
} from "../../api-types";
import { policyDeletesFiles, policyFromPrefs } from "../../helper/removePrefs";

export interface InterfaceTabProps {
  preferences: SessionPreferences;
  onChange: (patch: Partial<SessionPreferences>) => void;
}

export const selectClass =
  "w-full bg-surface border border-divider rounded px-2 py-1.5 text-sm";

export const InterfaceTab: React.FC<InterfaceTabProps> = ({
  preferences,
  onChange,
}) => {
  const confirm = preferences.confirm_remove !== false;
  const policy = policyFromPrefs(preferences);
  const deletes = policyDeletesFiles(policy);
  return (
    <div className="text-secondary py-2 space-y-4">
      <Fieldset label="Removing torrents">
        <FormCheckbox
          checked={confirm}
          name="confirm_remove"
          label="Confirm before removing torrents"
          help="Show the confirmation dialog for Remove (toolbar, row button, right-click menu and the Delete key). Saved on the server; applies to the web UI and the GPUI client."
          onChange={(e) => onChange({ confirm_remove: e.target.checked })}
        />
        <div className="mb-3">
          <label
            htmlFor="remove_policy_complete"
            className="block text-sm text-text mb-1"
          >
            When the torrent is complete
          </label>
          <select
            id="remove_policy_complete"
            className={selectClass}
            value={policy.complete}
            onChange={(e) =>
              onChange({
                remove_policy: {
                  ...policy,
                  complete: e.target.value as CompleteRemoveAction,
                },
              })
            }
          >
            <option value="keep">Keep files</option>
            <option value="delete">Delete files</option>
          </select>
        </div>
        <div className="mb-3">
          <label
            htmlFor="remove_policy_incomplete"
            className="block text-sm text-text mb-1"
          >
            When the torrent is incomplete
          </label>
          <select
            id="remove_policy_incomplete"
            className={selectClass}
            value={policy.incomplete}
            onChange={(e) =>
              onChange({
                remove_policy: {
                  ...policy,
                  incomplete: e.target.value as IncompleteRemoveAction,
                },
              })
            }
          >
            <option value="keep">Keep files (partial files stay on disk)</option>
            <option value="delete">Delete all files</option>
            <option value="finish">Finish what's done</option>
          </select>
          <p className="text-sm text-tertiary mt-1">
            <b>Finish what's done</b>: unfinished files are deselected and their
            partial data deleted (only files this torrent created), the
            completion actions (Preferences → Completion) run on the finished
            files, and then the torrent is removed with those files kept where
            the actions put them. If an action fails, the torrent is kept and
            flagged "needs attention". With no finished file at all, all partial
            data is deleted and the torrent removed.
          </p>
          <p className="text-sm text-tertiary mt-1">
            These preset the remove dialog, where they can be changed for a
            single removal. Every removal is written to the Events log.
          </p>
          {!confirm && deletes && (
            <p className="text-sm text-warning mt-1">
              Confirmation is off, but this policy can delete files, so the
              dialog is still shown whenever a removal would delete something.
            </p>
          )}
        </div>
      </Fieldset>
    </div>
  );
};
