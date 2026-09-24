import React, { useContext, useEffect, useState } from "react";
import { TabbedConfigModal } from "../modal/TabbedConfigModal";
import { RateLimitsTab } from "./RateLimitsTab";
import { DownloadsTab } from "./DownloadsTab";
import { OrganizeTab, DEFAULT_ORGANIZE_FOLDERS } from "./OrganizeTab";
import { CompletionTab } from "./CompletionTab";
import { AdminTab } from "./AdminTab";
import { APIContext } from "../../context";
import {
  LimitsConfig,
  SessionPreferences,
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
  on_complete_hook: "",
  move_completed_path: "",
  move_completed_copy: false,
  auto_organize_enabled: false,
  auto_organize_root: "",
  auto_organize_folders: { ...DEFAULT_ORGANIZE_FOLDERS },
  incomplete_extension: "",
  completion_actions: [],
});

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
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<ErrorWithLabel | null>(null);

  const API = useContext(APIContext);

  useEffect(() => {
    if (isOpen) {
      setLoading(true);
      setError(null);
      Promise.all([API.getLimits(), API.getPreferences()])
        .then(([config, prefs]) => {
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
        })
        .catch((e: ErrorDetails) => {
          setError({ text: "Error loading configuration", details: e });
        })
        .finally(() => setLoading(false));
    }
  }, [isOpen, API]);

  const patchPreferences = (patch: Partial<SessionPreferences>) =>
    setPreferences((p) => ({ ...p, ...patch }));

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
      });
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
          id: "limits",
          label: "Rate Limits",
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
          id: "admin",
          label: "Administration",
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
