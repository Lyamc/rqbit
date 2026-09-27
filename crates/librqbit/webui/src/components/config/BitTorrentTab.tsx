import React from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormInput } from "../forms/FormInput";
import { FormCheckbox } from "../forms/FormCheckbox";
import {
  AdminConfigPublic,
  AdminConfigUpdate,
  SessionPreferences,
} from "../../api-types";

export interface BitTorrentTabProps {
  admin: AdminConfigPublic;
  preferences: SessionPreferences;
  onAdminPatch: (patch: AdminConfigUpdate) => void;
  onPrefsChange: (patch: Partial<SessionPreferences>) => void;
}

type Tri = "default" | "on" | "off";

function triFromOptBool(v: boolean | null | undefined): Tri {
  if (v === null || v === undefined) return "default";
  return v ? "on" : "off";
}

export const BitTorrentTab: React.FC<BitTorrentTabProps> = ({
  admin,
  preferences,
  onAdminPatch,
  onPrefsChange,
}) => {
  return (
    <div className="text-secondary py-2 space-y-4">
      <Fieldset label="Queueing (live)">
        <FormCheckbox
          checked={!!preferences.queueing_enabled}
          name="queueing_enabled"
          label="Enable torrent queueing"
          help="Off by default (no limits). When on, torrents over the limits below are held as 'Queued' (not user-paused) in queue order and start automatically when a slot frees up. Reorder with the queue buttons in the torrent list."
          onChange={(e) => onPrefsChange({ queueing_enabled: e.target.checked })}
        />
        <FormInput
          name="queue_max_active_downloads"
          label="Maximum active downloads"
          inputType="number"
          value={preferences.queue_max_active_downloads?.toString() ?? ""}
          placeholder="empty = unlimited"
          disabled={!preferences.queueing_enabled}
          help="Torrents downloading at the same time."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!e.target.value) onPrefsChange({ queue_max_active_downloads: null });
            else if (!isNaN(v) && v >= 0)
              onPrefsChange({ queue_max_active_downloads: Math.floor(v) });
          }}
        />
        <FormInput
          name="queue_max_active_uploads"
          label="Maximum active uploads (seeding)"
          inputType="number"
          value={preferences.queue_max_active_uploads?.toString() ?? ""}
          placeholder="empty = unlimited"
          disabled={!preferences.queueing_enabled}
          help="Finished torrents seeding at the same time."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!e.target.value) onPrefsChange({ queue_max_active_uploads: null });
            else if (!isNaN(v) && v >= 0)
              onPrefsChange({ queue_max_active_uploads: Math.floor(v) });
          }}
        />
        <FormInput
          name="queue_max_active_torrents"
          label="Maximum active torrents"
          inputType="number"
          value={preferences.queue_max_active_torrents?.toString() ?? ""}
          placeholder="empty = unlimited"
          disabled={!preferences.queueing_enabled}
          help="Downloading + seeding in total."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!e.target.value) onPrefsChange({ queue_max_active_torrents: null });
            else if (!isNaN(v) && v >= 0)
              onPrefsChange({ queue_max_active_torrents: Math.floor(v) });
          }}
        />
        <FormCheckbox
          checked={!!preferences.queue_ignore_slow_torrents}
          name="queue_ignore_slow_torrents"
          label="Don't count slow torrents in these limits"
          disabled={!preferences.queueing_enabled}
          help="Torrents below 2 KiB/s download and upload for 60 s keep running but don't take a slot."
          onChange={(e) =>
            onPrefsChange({ queue_ignore_slow_torrents: e.target.checked })
          }
        />
      </Fieldset>

      <Fieldset label="Peers (live)">
        <FormInput
          name="peer_limit_live"
          label="Default peers per torrent"
          inputType="number"
          value={preferences.peer_limit?.toString() ?? ""}
          placeholder="empty = engine default"
          help="Applied immediately to newly added torrents and saved in preferences.json. Does not change already-running torrents' limits."
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!e.target.value) {
              onPrefsChange({ peer_limit: null });
            } else if (!isNaN(v) && v > 0) {
              onPrefsChange({ peer_limit: Math.floor(v) });
            }
          }}
        />
      </Fieldset>

      <Fieldset label="Peers / init (restart via admin.json)">
        <p className="text-sm text-tertiary mb-2">
          Startup defaults when env vars are unset. Prefer the live peer limit
          above unless you need the value before preferences load.
        </p>
        <FormInput
          name="peer_limit_admin"
          label="Startup peer limit override"
          inputType="number"
          value={admin.peer_limit?.toString() ?? ""}
          placeholder="empty = no admin override"
          onChange={(e) => {
            if (!e.target.value) onAdminPatch({ clear_peer_limit: true });
            else {
              const v = e.target.valueAsNumber;
              if (!isNaN(v) && v > 0) onAdminPatch({ peer_limit: Math.floor(v) });
            }
          }}
        />
        <FormInput
          name="concurrent_init_limit"
          label="Concurrent torrent initializations"
          inputType="number"
          value={admin.concurrent_init_limit?.toString() ?? ""}
          placeholder="empty = startup default (often 3–8)"
          help="How many torrents can hash/check in parallel at once."
          onChange={(e) => {
            if (!e.target.value)
              onAdminPatch({ clear_concurrent_init_limit: true });
            else {
              const v = e.target.valueAsNumber;
              if (!isNaN(v) && v > 0)
                onAdminPatch({ concurrent_init_limit: Math.floor(v) });
            }
          }}
        />
        <FormInput
          name="peer_connect_timeout_secs"
          label="Peer connect timeout (seconds)"
          inputType="number"
          value={admin.peer_connect_timeout_secs?.toString() ?? ""}
          placeholder="empty = startup default"
          onChange={(e) => {
            if (!e.target.value)
              onAdminPatch({ clear_peer_connect_timeout_secs: true });
            else {
              const v = e.target.valueAsNumber;
              if (!isNaN(v) && v > 0)
                onAdminPatch({ peer_connect_timeout_secs: Math.floor(v) });
            }
          }}
        />
        <FormInput
          name="peer_read_write_timeout_secs"
          label="Peer read/write timeout (seconds)"
          inputType="number"
          value={admin.peer_read_write_timeout_secs?.toString() ?? ""}
          placeholder="empty = startup default"
          onChange={(e) => {
            if (!e.target.value)
              onAdminPatch({ clear_peer_read_write_timeout_secs: true });
            else {
              const v = e.target.valueAsNumber;
              if (!isNaN(v) && v > 0)
                onAdminPatch({
                  peer_read_write_timeout_secs: Math.floor(v),
                });
            }
          }}
        />
      </Fieldset>

      <Fieldset label="Lists / resume (restart)">
        <FormInput
          name="blocklist_url"
          label="IP blocklist URL"
          value={admin.blocklist_url ?? ""}
          placeholder="https://… (empty = none)"
          help="P2P blocklist loaded at startup."
          onChange={(e) => onAdminPatch({ blocklist_url: e.target.value })}
        />
        <FormInput
          name="allowlist_url"
          label="IP allowlist URL"
          value={admin.allowlist_url ?? ""}
          placeholder="https://… (empty = none)"
          help="If set, only listed IPs may connect."
          onChange={(e) => onAdminPatch({ allowlist_url: e.target.value })}
        />
        <div className="mb-3">
          <label className="block text-sm text-text mb-1">Fastresume</label>
          <select
            className="w-full bg-surface border border-divider rounded px-2 py-1.5 text-sm"
            value={triFromOptBool(admin.fastresume)}
            onChange={(e) => {
              const t = e.target.value as Tri;
              if (t === "default") onAdminPatch({ clear_fastresume: true });
              else onAdminPatch({ fastresume: t === "on" });
            }}
          >
            <option value="default">Use startup default (CLI/env)</option>
            <option value="on">Enabled</option>
            <option value="off">Disabled</option>
          </select>
          <p className="text-sm text-tertiary mt-1">
            Faster session restore after restart by trusting stored piece bitfields.
          </p>
        </div>
      </Fieldset>

      <Fieldset label="Not available in rqbit yet">
        <ul className="text-sm list-disc pl-5 space-y-1 text-tertiary">
          <li>Protocol encryption (MSE/PE)</li>
          <li>Seeding ratio or seed-time limits</li>
          <li>Download queue / max active downloads</li>
          <li>Sequential download as a session default</li>
          <li>Disk preallocation mode toggle</li>
          <li>Disable PeX (peer exchange is used automatically for public torrents)</li>
        </ul>
      </Fieldset>
    </div>
  );
};
