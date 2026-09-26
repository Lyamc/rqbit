import React from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormCheckbox } from "../forms/FormCheckbox";
import { RemoveAction, SessionPreferences } from "../../api-types";

export interface InterfaceTabProps {
  preferences: SessionPreferences;
  onChange: (patch: Partial<SessionPreferences>) => void;
}

export const InterfaceTab: React.FC<InterfaceTabProps> = ({
  preferences,
  onChange,
}) => {
  const confirm = preferences.confirm_remove !== false;
  const action: RemoveAction =
    preferences.default_remove_action ?? "keep_files";
  return (
    <div className="text-secondary py-2 space-y-4">
      <Fieldset label="Removing torrents">
        <FormCheckbox
          checked={confirm}
          name="confirm_remove"
          label="Confirm before removing torrents"
          help="Show the confirmation dialog for Remove / Delete (toolbar, row button and the Delete key). When off, torrents are removed right away and their files are kept. Saved on the server; applies to the web UI and the GPUI client."
          onChange={(e) => onChange({ confirm_remove: e.target.checked })}
        />
        <div className="mb-3">
          <label
            htmlFor="default_remove_action"
            className="block text-sm text-text mb-1"
          >
            Default remove action
          </label>
          <select
            id="default_remove_action"
            className="w-full bg-surface border border-divider rounded px-2 py-1.5 text-sm"
            value={action}
            onChange={(e) =>
              onChange({
                default_remove_action: e.target.value as RemoveAction,
              })
            }
          >
            <option value="keep_files">Remove torrent only (keep files)</option>
            <option value="delete_files">
              Remove torrent and delete files
            </option>
          </select>
          <p className="text-sm text-tertiary mt-1">
            Presets the dialog's "Also delete downloaded files" checkbox.
            Deleting files always asks for confirmation, even when confirmation
            is turned off above.
          </p>
          {!confirm && action === "delete_files" && (
            <p className="text-sm text-warning mt-1">
              Confirmation is off, but because the default deletes files the
              dialog will still be shown.
            </p>
          )}
        </div>
      </Fieldset>
    </div>
  );
};
