import React from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormCheckbox } from "../forms/FormCheckbox";
import { FormInput } from "../forms/FormInput";
import {
  DownloadOrderDefaults,
  FileOrder,
  SessionPreferences,
} from "../../api-types";

export const DEFAULT_DOWNLOAD_ORDER: DownloadOrderDefaults = {
  sequential_files: true,
  file_order: "name",
  sequential: true,
  first_last_first: true,
};

export const FILE_ORDER_LABELS: Record<FileOrder, string> = {
  name: "By name (default)",
  torrent: "Torrent order",
  smallest_first: "Smallest first",
  largest_first: "Largest first",
};

export interface DownloadsTabProps {
  preferences: SessionPreferences;
  onChange: (patch: Partial<SessionPreferences>) => void;
}

export const DownloadsTab: React.FC<DownloadsTabProps> = ({
  preferences,
  onChange,
}) => {
  const order = preferences.download_order ?? DEFAULT_DOWNLOAD_ORDER;
  const setOrder = (patch: Partial<DownloadOrderDefaults>) =>
    onChange({ download_order: { ...order, ...patch } });
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
          help="Needs soft-recover. When a file keeps failing with I/O errors (e.g. unreadable extents on the filesystem), automatically punch out the unreadable ranges (or copy-and-replace the file) and redownload only the affected pieces. Retries use the backoff below. Off by default."
          onChange={(e) =>
            onChange({ auto_repair_damaged_files: e.target.checked })
          }
        />
        <FormInput
          name="recovery_backoff_base_secs"
          label="Retry delay after an I/O error (seconds)"
          inputType="number"
          value={(preferences.recovery_backoff_base_secs ?? 60).toString()}
          help="Automatic recovery (re-downloading a piece that failed with a disk error, automatic repair) waits this long before the first retry, doubling after each consecutive failure (±20% jitter)."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!isNaN(v) && v >= 1)
              onChange({ recovery_backoff_base_secs: Math.floor(v) });
          }}
        />
        <FormInput
          name="recovery_backoff_cap_secs"
          label="Maximum retry delay (seconds)"
          inputType="number"
          value={(preferences.recovery_backoff_cap_secs ?? 21600).toString()}
          help="Upper bound for the doubling delay (default 21600 = 6 h)."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!isNaN(v) && v >= 1)
              onChange({ recovery_backoff_cap_secs: Math.floor(v) });
          }}
        />
        <FormInput
          name="recovery_max_attempts"
          label="Give up after N consecutive failures"
          inputType="number"
          value={(preferences.recovery_max_attempts ?? 8).toString()}
          help="Then automatic retries stop for that piece/file and the torrent shows 'needs attention'. Fix errors always retries immediately and resets the counters. Counters reset when rqbit restarts."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!isNaN(v) && v >= 1)
              onChange({ recovery_max_attempts: Math.floor(v) });
          }}
        />
        <FormInput
          name="event_log_max_mb"
          label="Event log size limit (MB)"
          inputType="number"
          value={(preferences.event_log_max_mb ?? 10).toString()}
          help="Repairs, recovery failures and (rate-limited) I/O errors are kept in events.jsonl next to preferences.json, rotated so all segments together never exceed this size; the oldest events are dropped first. 1–1024, default 10."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!isNaN(v) && v >= 1 && v <= 1024)
              onChange({ event_log_max_mb: Math.floor(v) });
          }}
        />
      </Fieldset>

      <Fieldset label="Download order (defaults)">
        <p className="text-sm text-tertiary mb-2">
          Order in which pieces are requested. Each torrent can override these
          (right-click a torrent or files → Download order), and single files
          can override the torrent. Applied immediately to running torrents.
        </p>
        <FormCheckbox
          checked={order.sequential_files}
          name="do_sequential_files"
          label="Sequential file download"
          help="Finish files one after another in the file order below. Off: work on all selected files at once."
          onChange={(e) => setOrder({ sequential_files: e.target.checked })}
        />
        <div className="flex flex-col gap-1 mb-2">
          <label htmlFor="do_file_order" className="text-sm">
            File order
          </label>
          <select
            id="do_file_order"
            className="w-full bg-surface border border-divider rounded px-2 py-1.5 text-sm"
            value={order.file_order}
            onChange={(e) =>
              setOrder({ file_order: e.target.value as FileOrder })
            }
          >
            {(Object.keys(FILE_ORDER_LABELS) as FileOrder[]).map((k) => (
              <option key={k} value={k}>
                {FILE_ORDER_LABELS[k]}
              </option>
            ))}
          </select>
        </div>
        <FormCheckbox
          checked={order.sequential}
          name="do_sequential"
          label="Sequential download (within each file)"
          help="Request a file's pieces in order. Off: spread requests across the file."
          onChange={(e) => setOrder({ sequential: e.target.checked })}
        />
        <FormCheckbox
          checked={order.first_last_first}
          name="do_first_last"
          label="Download first and last pieces first"
          help="Fetch each file's first and last pieces early (media headers / previews)."
          onChange={(e) => setOrder({ first_last_first: e.target.checked })}
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
