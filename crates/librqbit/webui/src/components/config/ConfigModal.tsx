import React, { useContext, useEffect, useState } from "react";
import { TabbedConfigModal } from "../modal/TabbedConfigModal";
import { RateLimitsTab } from "./RateLimitsTab";
import { APIContext } from "../../context";
import {
  LimitsConfig,
  SessionPreferences,
  CompletionAction,
  AutoOrganizeFolders,
  ErrorDetails,
} from "../../api-types";
import { FormCheckbox } from "../forms/FormCheckbox";
import { FormInput } from "../forms/FormInput";
import { ErrorWithLabel } from "../../rqbit-web";
import { Spinner } from "../Spinner";
import { Modal } from "../modal/Modal";
import { ModalBody } from "../modal/ModalBody";

export interface ConfigModalProps {
  isOpen: boolean;
  onClose: () => void;
}

const DEFAULT_FOLDERS: AutoOrganizeFolders = {
  anime: "Anime",
  tv: "TV",
  movie: "Movies",
  game: "Games",
  porn: "Adult",
  music: "Music",
  book: "Books",
  software: "Software",
  other: "Other",
};

const defaultPreferences = (): SessionPreferences => ({
  soft_recover_on_io_error: false,
  on_complete_hook: "",
  move_completed_path: "",
  move_completed_copy: false,
  auto_organize_enabled: false,
  auto_organize_root: "",
  auto_organize_folders: { ...DEFAULT_FOLDERS },
  incomplete_extension: "",
  completion_actions: [],
});

function actionLabel(a: CompletionAction): string {
  switch (a.type) {
    case "shell":
      return `Shell: ${a.command || "(empty)"}`;
    case "move":
      return `Move${a.copy ? " (copy)" : ""}: ${a.path || "(empty)"}`;
    case "organize":
      return "Auto-organize";
    case "drop_incomplete_ext":
      return "Drop incomplete extension";
    default:
      return a.type;
  }
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
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<ErrorWithLabel | null>(null);
  const [newActionType, setNewActionType] =
    useState<CompletionAction["type"]>("shell");

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
              ...DEFAULT_FOLDERS,
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

  const actions = preferences.completion_actions || [];

  const setActions = (completion_actions: CompletionAction[]) =>
    setPreferences((p) => ({ ...p, completion_actions }));

  const moveAction = (index: number, dir: -1 | 1) => {
    const next = [...actions];
    const j = index + dir;
    if (j < 0 || j >= next.length) return;
    const tmp = next[index];
    next[index] = next[j];
    next[j] = tmp;
    setActions(next);
  };

  const removeAction = (index: number) => {
    setActions(actions.filter((_, i) => i !== index));
  };

  const addAction = () => {
    let a: CompletionAction;
    switch (newActionType) {
      case "shell":
        a = { type: "shell", command: "" };
        break;
      case "move":
        a = { type: "move", path: "", copy: false };
        break;
      case "organize":
        a = { type: "organize" };
        break;
      case "drop_incomplete_ext":
        a = { type: "drop_incomplete_ext" };
        break;
    }
    setActions([...actions, a]);
  };

  const updateAction = (index: number, patch: Partial<CompletionAction>) => {
    setActions(
      actions.map((a, i) => (i === index ? { ...a, ...patch } : a)),
    );
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
        completion_actions: actions,
      });
      onClose();
    } catch (e) {
      setError({ text: "Error saving configuration", details: e as ErrorDetails });
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

  const folderKeys = Object.keys(DEFAULT_FOLDERS) as (keyof AutoOrganizeFolders)[];

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
          id: "other",
          label: "Other",
          content: (
            <div className="text-secondary py-2 space-y-3">
              <FormCheckbox
                checked={preferences.soft_recover_on_io_error}
                name="soft_recover_on_io_error"
                label="Soft-recover on disk I/O errors"
                help="When a write fails, invalidate only the affected piece and redownload it instead of fatally stopping the torrent. Saved on the server."
                onChange={(e) =>
                  setPreferences((p) => ({
                    ...p,
                    soft_recover_on_io_error: e.target.checked,
                  }))
                }
              />

              <FormInput
                name="incomplete_extension"
                label="Incomplete file extension"
                value={preferences.incomplete_extension ?? ""}
                placeholder="e.g. .!qB or .part (empty = disabled)"
                help="While downloading, on-disk names get this suffix. A DropIncompleteExt action (or legacy auto-pipeline) removes it when the torrent finishes."
                onChange={(e) =>
                  setPreferences((p) => ({
                    ...p,
                    incomplete_extension: e.target.value || null,
                  }))
                }
              />

              <FormCheckbox
                checked={!!preferences.auto_organize_enabled}
                name="auto_organize_enabled"
                label="Auto-organize completed torrents"
                help="DISABLED by default. When enabled (and no custom action list), finished torrents are classified (anime/tv/movie/game/porn/music/book/software/other) from name + filenames and moved under the organize root. Heuristics can be wrong."
                onChange={(e) =>
                  setPreferences((p) => ({
                    ...p,
                    auto_organize_enabled: e.target.checked,
                  }))
                }
              />
              <FormInput
                name="auto_organize_root"
                label="Auto-organize root"
                value={preferences.auto_organize_root ?? ""}
                placeholder="(empty = session download folder)"
                help="Base directory for type subfolders. Destination becomes root/<type>/<torrent-folder>."
                onChange={(e) =>
                  setPreferences((p) => ({
                    ...p,
                    auto_organize_root: e.target.value || null,
                  }))
                }
              />
              <div className="border border-divider rounded p-2 space-y-2">
                <div className="text-sm font-medium">Type → subfolder names</div>
                {folderKeys.map((key) => (
                  <FormInput
                    key={key}
                    name={`folder_${key}`}
                    label={key}
                    value={preferences.auto_organize_folders?.[key] ?? DEFAULT_FOLDERS[key]}
                    onChange={(e) =>
                      setPreferences((p) => ({
                        ...p,
                        auto_organize_folders: {
                          ...(p.auto_organize_folders || DEFAULT_FOLDERS),
                          [key]: e.target.value,
                        },
                      }))
                    }
                  />
                ))}
              </div>

              <div className="border-t border-divider pt-3 space-y-2">
                <div className="font-medium">Completion action pipeline</div>
                <p className="text-sm">
                  Ordered actions run when a torrent finishes. If the list is
                  empty, rqbit synthesizes actions from the legacy fields below
                  plus incomplete-ext / auto-organize toggles (drop incomplete →
                  organize → move → shell).
                </p>
                {actions.length === 0 && (
                  <p className="text-sm italic">
                    No explicit actions — using legacy / toggle synthesis.
                  </p>
                )}
                <ul className="space-y-2">
                  {actions.map((a, i) => (
                    <li
                      key={i}
                      className="border border-divider rounded p-2 space-y-2"
                    >
                      <div className="flex items-center gap-2 flex-wrap">
                        <span className="text-sm font-mono">{i + 1}.</span>
                        <span className="text-sm">{actionLabel(a)}</span>
                        <button
                          type="button"
                          className="text-sm px-2 py-0.5 border border-divider rounded"
                          onClick={() => moveAction(i, -1)}
                          disabled={i === 0}
                        >
                          Up
                        </button>
                        <button
                          type="button"
                          className="text-sm px-2 py-0.5 border border-divider rounded"
                          onClick={() => moveAction(i, 1)}
                          disabled={i === actions.length - 1}
                        >
                          Down
                        </button>
                        <button
                          type="button"
                          className="text-sm px-2 py-0.5 border border-divider rounded text-red-500"
                          onClick={() => removeAction(i)}
                        >
                          Remove
                        </button>
                      </div>
                      {a.type === "shell" && (
                        <FormInput
                          name={`action_shell_${i}`}
                          label="Command"
                          value={a.command ?? ""}
                          placeholder="notify-send done $RQBIT_NAME"
                          onChange={(e) =>
                            updateAction(i, { command: e.target.value })
                          }
                        />
                      )}
                      {a.type === "move" && (
                        <>
                          <FormInput
                            name={`action_move_${i}`}
                            label="Destination"
                            value={a.path ?? ""}
                            placeholder="/data/completed"
                            onChange={(e) =>
                              updateAction(i, { path: e.target.value })
                            }
                          />
                          <FormCheckbox
                            checked={!!a.copy}
                            name={`action_move_copy_${i}`}
                            label="Copy instead of move"
                            onChange={(e) =>
                              updateAction(i, { copy: e.target.checked })
                            }
                          />
                        </>
                      )}
                    </li>
                  ))}
                </ul>
                <div className="flex items-center gap-2 flex-wrap">
                  <select
                    className="border border-divider rounded bg-transparent py-1 px-2"
                    value={newActionType}
                    onChange={(e) =>
                      setNewActionType(
                        e.target.value as CompletionAction["type"],
                      )
                    }
                  >
                    <option value="shell">Shell hook</option>
                    <option value="move">Move / copy</option>
                    <option value="organize">Auto-organize</option>
                    <option value="drop_incomplete_ext">
                      Drop incomplete extension
                    </option>
                  </select>
                  <button
                    type="button"
                    className="px-3 py-1 border border-divider rounded"
                    onClick={addAction}
                  >
                    Add action
                  </button>
                </div>
              </div>

              <div className="border-t border-divider pt-3 space-y-2">
                <div className="font-medium">Legacy fields (used when action list is empty)</div>
                <FormInput
                  name="on_complete_hook"
                  label="On-complete hook (shell)"
                  value={preferences.on_complete_hook ?? ""}
                  placeholder="e.g. notify-send done $RQBIT_NAME"
                  help="Shell command run when a torrent finishes. Env: RQBIT_TORRENT_ID, RQBIT_INFO_HASH, RQBIT_NAME, RQBIT_OUTPUT_FOLDER."
                  onChange={(e) =>
                    setPreferences((p) => ({
                      ...p,
                      on_complete_hook: e.target.value || null,
                    }))
                  }
                />
                <FormInput
                  name="move_completed_path"
                  label="Move completed to"
                  value={preferences.move_completed_path ?? ""}
                  placeholder="/data/completed"
                  help="If set, move (or copy) torrent files here when download finishes. Seeding continues from the new location."
                  onChange={(e) =>
                    setPreferences((p) => ({
                      ...p,
                      move_completed_path: e.target.value || null,
                    }))
                  }
                />
                <FormCheckbox
                  checked={!!preferences.move_completed_copy}
                  name="move_completed_copy"
                  label="Copy instead of move when completing"
                  help="Leave originals in place and copy into the completed folder."
                  onChange={(e) =>
                    setPreferences((p) => ({
                      ...p,
                      move_completed_copy: e.target.checked,
                    }))
                  }
                />
              </div>

              <p>
                All other parameters (DHT, connections, persistence, etc.) can
                be configured via{" "}
                <code className="bg-surface-sunken px-1 rounded text-sm">
                  rqbit
                </code>{" "}
                CLI arguments when starting the server.
              </p>
            </div>
          ),
        },
      ]}
      onSave={handleSave}
      isSaving={saving}
      error={error}
      showResetButton={false}
    />
  );
};
