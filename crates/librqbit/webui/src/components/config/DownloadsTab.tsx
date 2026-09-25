import React from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormCheckbox } from "../forms/FormCheckbox";
import { FormInput } from "../forms/FormInput";
import { SessionPreferences } from "../../api-types";

export interface DownloadsTabProps {
  preferences: SessionPreferences;
  onChange: (patch: Partial<SessionPreferences>) => void;
}

export const DownloadsTab: React.FC<DownloadsTabProps> = ({
  preferences,
  onChange,
}) => {
  return (
    <div className="text-secondary py-2 space-y-4">
      <Fieldset label="Reliability">
        <FormCheckbox
          checked={preferences.soft_recover_on_io_error}
          name="soft_recover_on_io_error"
          label="Soft-recover on disk I/O errors"
          help="When a write fails, invalidate only the affected piece and redownload it instead of fatally stopping the torrent. Saved on the server."
          onChange={(e) =>
            onChange({ soft_recover_on_io_error: e.target.checked })
          }
        />
        <FormCheckbox
          checked={!!preferences.auto_repair_damaged_files}
          name="auto_repair_damaged_files"
          label="Auto-repair damaged files"
          help="Needs soft-recover. When a file keeps failing with I/O errors (e.g. unreadable extents on the filesystem), automatically punch out the unreadable ranges (or copy-and-replace the file) and redownload only the affected pieces. At most once per 30 min per torrent. Off by default."
          onChange={(e) =>
            onChange({ auto_repair_damaged_files: e.target.checked })
          }
        />
      </Fieldset>

      <Fieldset label="Incomplete files">
        <FormInput
          name="incomplete_extension"
          label="Incomplete file extension"
          value={preferences.incomplete_extension ?? ""}
          placeholder="e.g. .!qB or .part (empty = disabled)"
          help="While downloading, on-disk names get this suffix. A DropIncompleteExt action (or legacy auto-pipeline) removes it when the torrent finishes."
          onChange={(e) =>
            onChange({ incomplete_extension: e.target.value || null })
          }
        />
      </Fieldset>

      <p className="text-sm text-tertiary">
        Connection, DHT/LSD/trackers, ports, and proxy settings live under the{" "}
        <strong className="text-text">Connection</strong> tab (admin.json,
        restart required). Peer limits and timeouts are under{" "}
        <strong className="text-text">BitTorrent</strong>.
      </p>
    </div>
  );
};
