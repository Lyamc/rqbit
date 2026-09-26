import React, { useContext, useEffect, useState } from "react";
import { TabbedConfigModal } from "../modal/TabbedConfigModal";
import { RateLimitsTab } from "./RateLimitsTab";
import { DownloadsTab } from "./DownloadsTab";
import { OrganizeTab, DEFAULT_ORGANIZE_FOLDERS } from "./OrganizeTab";
import { CompletionTab } from "./CompletionTab";
import { AdminTab } from "./AdminTab";
import { ConnectionTab } from "./ConnectionTab";
import { BitTorrentTab } from "./BitTorrentTab";
import { InterfaceTab } from "./InterfaceTab";
import { usePrefsStore } from "../../stores/prefsStore";
import { APIContext } from "../../context";
import {
  LimitsConfig,
  SessionPreferences,
  AdminConfigPublic,
  AdminConfigUpdate,
  ErrorDetails,
} from "../../api-types";
import { ErrorWithLabel } from "../../rqbit-web";
import { Spinner } from "../Spinner";
import { Modal } from "../modal/Modal";
import { ModalBody } from "../modal/ModalBody";

export interface ConfigModalProps {
  isOpen: boolean;
  onClose: () => void;
}

const defaultPreferences = (): SessionPreferences => ({
  soft_recover_on_io_error: false,
  auto_repair_damaged_files: false,
  event_log_max_mb: 10,
  recovery_backoff_base_secs: 60,
  recovery_backoff_cap_secs: 21600,
  recovery_max_attempts: 8,
  queueing_enabled: false,
  queue_max_active_downloads: null,
  queue_max_active_uploads: null,
  queue_max_active_torrents: null,
  queue_ignore_slow_torrents: false,
  on_complete_hook: "",
  move_completed_path: "",
  move_completed_copy: false,
  auto_organize_enabled: false,
  auto_organize_root: "",
  auto_organize_folders: { ...DEFAULT_ORGANIZE_FOLDERS },
  incomplete_extension: "",
  completion_actions: [],
  peer_limit: null,
  confirm_remove: true,
  default_remove_action: "keep_files",
});

const emptyAdmin = (): AdminConfigPublic => ({
  basic_auth_enabled: false,
  basic_auth_password_set: false,
});

/** Merge UI admin patches into a cumulative update sent on Save. */
function mergeAdminPatch(
  prev: AdminConfigUpdate,
  patch: AdminConfigUpdate,
): AdminConfigUpdate {
  return { ...prev, ...patch };
}

export const ConfigModal: React.FC<ConfigModalProps> = ({
  isOpen,
  onClose,
}) => {
  const [limits, setLimits] = useState<LimitsConfig>({
    upload_bps: null,
    download_bps: null,
  });
  const [preferences, setPreferences] =
    useState<SessionPreferences>(defaultPreferences());
  const [adminView, setAdminView] = useState<AdminConfigPublic>(emptyAdmin());
  const [adminPatch, setAdminPatch] = useState<AdminConfigUpdate>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<ErrorWithLabel | null>(null);

  const API = useContext(APIContext);

  useEffect(() => {
    if (isOpen) {
      setLoading(true);
      setError(null);
      setAdminPatch({});
      Promise.all([
        API.getLimits(),
        API.getPreferences(),
        API.getAdminStatus(),
      ])
        .then(([config, prefs, status]) => {
          setLimits(config);
          setPreferences({
            ...defaultPreferences(),
            ...prefs,
            auto_organize_folders: {
              ...DEFAULT_ORGANIZE_FOLDERS,
              ...(prefs.auto_organize_folders || {}),
            },
            completion_actions: prefs.completion_actions || [],
          });
          setAdminView(status.persisted || emptyAdmin());
        })
        .catch((e: ErrorDetails) => {
          setError({ text: "Error loading configuration", details: e });
        })
        .finally(() => setLoading(false));
    }
  }, [isOpen, API]);

  const patchPreferences = (patch: Partial<SessionPreferences>) =>
    setPreferences((p) => ({ ...p, ...patch }));

  const patchAdmin = (patch: AdminConfigUpdate) => {
    setAdminPatch((prev) => mergeAdminPatch(prev, patch));
    // Optimistic local view for controls
    setAdminView((prev) => {
      const next = { ...prev };
      const assign = <K extends keyof AdminConfigPublic>(
        key: K,
        value: AdminConfigPublic[K],
      ) => {
        next[key] = value;
      };
      if (patch.listen_port !== undefined) assign("listen_port", patch.listen_port);
      if (patch.clear_listen_port) assign("listen_port", null);
      if (patch.announce_port !== undefined)
        assign("announce_port", patch.announce_port);
      if (patch.clear_announce_port) assign("announce_port", null);
      if (patch.disable_dht !== undefined) assign("disable_dht", patch.disable_dht);
      if (patch.clear_disable_dht) assign("disable_dht", null);
      if (patch.disable_dht_persistence !== undefined)
        assign("disable_dht_persistence", patch.disable_dht_persistence);
      if (patch.clear_disable_dht_persistence)
        assign("disable_dht_persistence", null);
      if (patch.disable_lsd !== undefined) assign("disable_lsd", patch.disable_lsd);
      if (patch.clear_disable_lsd) assign("disable_lsd", null);
      if (patch.disable_trackers !== undefined)
        assign("disable_trackers", patch.disable_trackers);
      if (patch.clear_disable_trackers) assign("disable_trackers", null);
      if (patch.enable_utp_listen !== undefined)
        assign("enable_utp_listen", patch.enable_utp_listen);
      if (patch.clear_enable_utp_listen) assign("enable_utp_listen", null);
      if (patch.disable_tcp_listen !== undefined)
        assign("disable_tcp_listen", patch.disable_tcp_listen);
      if (patch.clear_disable_tcp_listen) assign("disable_tcp_listen", null);
      if (patch.disable_tcp_connect !== undefined)
        assign("disable_tcp_connect", patch.disable_tcp_connect);
      if (patch.clear_disable_tcp_connect) assign("disable_tcp_connect", null);
      if (patch.disable_upnp_port_forward !== undefined)
        assign("disable_upnp_port_forward", patch.disable_upnp_port_forward);
      if (patch.clear_disable_upnp_port_forward)
        assign("disable_upnp_port_forward", null);
      if (patch.socks_proxy_url !== undefined)
        assign("socks_proxy_url", patch.socks_proxy_url || null);
      if (patch.ipv4_only !== undefined) assign("ipv4_only", patch.ipv4_only);
      if (patch.clear_ipv4_only) assign("ipv4_only", null);
      if (patch.bind_device !== undefined)
        assign("bind_device", patch.bind_device || null);
      if (patch.peer_limit !== undefined) assign("peer_limit", patch.peer_limit);
      if (patch.clear_peer_limit) assign("peer_limit", null);
      if (patch.concurrent_init_limit !== undefined)
        assign("concurrent_init_limit", patch.concurrent_init_limit);
      if (patch.clear_concurrent_init_limit)
        assign("concurrent_init_limit", null);
      if (patch.peer_connect_timeout_secs !== undefined)
        assign("peer_connect_timeout_secs", patch.peer_connect_timeout_secs);
      if (patch.clear_peer_connect_timeout_secs)
        assign("peer_connect_timeout_secs", null);
      if (patch.peer_read_write_timeout_secs !== undefined)
        assign(
          "peer_read_write_timeout_secs",
          patch.peer_read_write_timeout_secs,
        );
      if (patch.clear_peer_read_write_timeout_secs)
        assign("peer_read_write_timeout_secs", null);
      if (patch.blocklist_url !== undefined)
        assign("blocklist_url", patch.blocklist_url || null);
      if (patch.allowlist_url !== undefined)
        assign("allowlist_url", patch.allowlist_url || null);
      if (patch.fastresume !== undefined) assign("fastresume", patch.fastresume);
      if (patch.clear_fastresume) assign("fastresume", null);
      return next;
    });
  };

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    try {
      await API.setLimits(limits);
      await API.setPreferences({
        ...preferences,
        on_complete_hook: preferences.on_complete_hook?.trim()
          ? preferences.on_complete_hook
          : null,
        move_completed_path: preferences.move_completed_path?.trim()
          ? preferences.move_completed_path
          : null,
        auto_organize_root: preferences.auto_organize_root?.trim()
          ? preferences.auto_organize_root
          : null,
        incomplete_extension: preferences.incomplete_extension?.trim()
          ? preferences.incomplete_extension
          : null,
        completion_actions: preferences.completion_actions || [],
        peer_limit: preferences.peer_limit || null,
      });
      if (Object.keys(adminPatch).length > 0) {
        await API.updateAdminConfig(adminPatch);
      }
      // Remove/delete behaviour follows the saved preferences right away.
      usePrefsStore.getState().setPreferences(preferences);
      onClose();
    } catch (e) {
      setError({
        text: "Error saving configuration",
        details: e as ErrorDetails,
      });
    } finally {
      setSaving(false);
    }
  };

  if (loading && isOpen) {
    return (
      <Modal isOpen={isOpen} onClose={onClose} title="Configure">
        <ModalBody>
          <div className="flex justify-center p-4">
            <Spinner />
          </div>
        </ModalBody>
      </Modal>
    );
  }

  return (
    <TabbedConfigModal
      isOpen={isOpen}
      onClose={onClose}
      title="Configure"
      tabs={[
        {
          id: "speed",
          label: "Speed",
          content: (
            <RateLimitsTab
              downloadBps={limits.download_bps}
              uploadBps={limits.upload_bps}
              onDownloadBpsChange={(v) =>
                setLimits((l) => ({ ...l, download_bps: v }))
              }
              onUploadBpsChange={(v) =>
                setLimits((l) => ({ ...l, upload_bps: v }))
              }
            />
          ),
        },
        {
          id: "connection",
          label: "Connection",
          content: (
            <ConnectionTab admin={adminView} onPatch={patchAdmin} />
          ),
        },
        {
          id: "bittorrent",
          label: "BitTorrent",
          content: (
            <BitTorrentTab
              admin={adminView}
              preferences={preferences}
              onAdminPatch={patchAdmin}
              onPrefsChange={patchPreferences}
            />
          ),
        },
        {
          id: "downloads",
          label: "Downloads",
          content: (
            <DownloadsTab
              preferences={preferences}
              onChange={patchPreferences}
            />
          ),
        },
        {
          id: "organize",
          label: "Organize",
          content: (
            <OrganizeTab
              preferences={preferences}
              onChange={patchPreferences}
            />
          ),
        },
        {
          id: "completion",
          label: "Completion",
          content: (
            <CompletionTab
              preferences={preferences}
              onChange={patchPreferences}
            />
          ),
        },
        {
          id: "interface",
          label: "Interface",
          content: (
            <InterfaceTab
              preferences={preferences}
              onChange={patchPreferences}
            />
          ),
        },
        {
          id: "admin",
          label: "Web UI / Admin",
          content: <AdminTab />,
        },
      ]}
      onSave={handleSave}
      isSaving={saving}
      error={error}
      showResetButton={false}
    />
  );
};
