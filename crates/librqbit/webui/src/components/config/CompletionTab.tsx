import React, { useState } from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormCheckbox } from "../forms/FormCheckbox";
import { FormInput } from "../forms/FormInput";
import { CompletionAction, SessionPreferences } from "../../api-types";
import { CollapsibleSection } from "./CollapsibleSection";

/** UI preset keys ? map onto CompletionAction types (+ optional defaults). */
type ActionPreset =
  | "move"
  | "organize"
  | "drop_incomplete_ext"
  | "shell_notify"
  | "shell_sound"
  | "shell_custom";

const PRESET_OPTIONS: { id: ActionPreset; label: string }[] = [
  { id: "move", label: "Move completed" },
  { id: "organize", label: "Auto-organize" },
  { id: "drop_incomplete_ext", label: "Drop incomplete extension" },
  { id: "shell_notify", label: "Desktop notification" },
  { id: "shell_sound", label: "Play sound" },
  { id: "shell_custom", label: "Custom shell?" },
];

const NOTIFY_CMD =
  'notify-send "rqbit" "Finished: $RQBIT_NAME"';
const SOUND_CMD =
  "paplay /usr/share/sounds/freedesktop/stereo/complete.oga 2>/dev/null || true";

function presetFromAction(a: CompletionAction): ActionPreset {
  if (a.type === "move") return "move";
  if (a.type === "organize") return "organize";
  if (a.type === "drop_incomplete_ext") return "drop_incomplete_ext";
  const cmd = (a.command ?? "").trim();
  if (cmd.startsWith("notify-send")) return "shell_notify";
  if (cmd.includes("paplay") || cmd.includes("afplay") || cmd.includes("play "))
    return "shell_sound";
  return "shell_custom";
}

function actionFromPreset(preset: ActionPreset, prev?: CompletionAction): CompletionAction {
  switch (preset) {
    case "move":
      return {
        type: "move",
        path: prev?.type === "move" ? prev.path ?? "" : "",
        copy: prev?.type === "move" ? !!prev.copy : false,
      };
    case "organize":
      return { type: "organize" };
    case "drop_incomplete_ext":
      return { type: "drop_incomplete_ext" };
    case "shell_notify":
      return {
        type: "shell",
        command:
          prev?.type === "shell" && (prev.command ?? "").startsWith("notify-send")
            ? prev.command!
            : NOTIFY_CMD,
      };
    case "shell_sound":
      return {
        type: "shell",
        command:
          prev?.type === "shell" &&
          ((prev.command ?? "").includes("paplay") ||
            (prev.command ?? "").includes("afplay"))
            ? prev.command!
            : SOUND_CMD,
      };
    case "shell_custom":
      return {
        type: "shell",
        command:
          prev?.type === "shell" && presetFromAction(prev) === "shell_custom"
            ? prev.command ?? ""
            : "",
      };
  }
}

const IconBtn: React.FC<{
  label: string;
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
  children: React.ReactNode;
}> = ({ label, onClick, disabled, danger, children }) => (
  <button
    type="button"
    title={label}
    aria-label={label}
    disabled={disabled}
    onClick={onClick}
    className={`inline-flex items-center justify-center w-8 h-8 rounded border border-divider text-sm font-medium transition-colors disabled:opacity-35 disabled:cursor-not-allowed hover:bg-surface-raised ${
      danger ? "text-error" : "text-secondary"
    }`}
  >
    {children}
  </button>
);

export interface CompletionTabProps {
  preferences: SessionPreferences;
  onChange: (patch: Partial<SessionPreferences>) => void;
}

export const CompletionTab: React.FC<CompletionTabProps> = ({
  preferences,
  onChange,
}) => {
  const actions = preferences.completion_actions || [];
  const [addPreset, setAddPreset] = useState<ActionPreset>("move");

  const setActions = (completion_actions: CompletionAction[]) =>
    onChange({ completion_actions });

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
    setActions([...actions, actionFromPreset(addPreset)]);
  };

  const changePreset = (index: number, preset: ActionPreset) => {
    setActions(
      actions.map((a, i) => (i === index ? actionFromPreset(preset, a) : a)),
    );
  };

  const updateAction = (index: number, patch: Partial<CompletionAction>) => {
    setActions(actions.map((a, i) => (i === index ? { ...a, ...patch } : a)));
  };

  const hasLegacy =
    !!(preferences.on_complete_hook && preferences.on_complete_hook.trim()) ||
    !!(
      preferences.move_completed_path &&
      preferences.move_completed_path.trim()
    ) ||
    !!preferences.move_completed_copy;

  return (
    <div className="text-secondary py-2 space-y-4">
      <Fieldset label="Action pipeline">
        <p className="text-sm mb-3">
          Ordered actions run when a torrent finishes. Pick a common action from
          the list, or choose <em>Custom shell?</em> for a freeform command. If
          the list is empty, rqbit synthesizes actions from legacy fields plus
          incomplete-ext / auto-organize toggles.
        </p>

        {actions.length === 0 && (
          <p className="text-sm italic text-tertiary mb-3">
            No explicit actions ? using legacy / toggle synthesis.
          </p>
        )}

        <ul className="space-y-2 mb-3">
          {actions.map((a, i) => {
            const preset = presetFromAction(a);
            return (
              <li
                key={i}
                className="border border-divider rounded p-2 space-y-2 bg-surface"
              >
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="text-sm font-mono text-tertiary w-5 shrink-0">
                    {i + 1}.
                  </span>
                  <select
                    className="flex-1 min-w-[12rem] border border-divider rounded bg-surface py-1.5 px-2 text-text text-sm"
                    value={preset}
                    onChange={(e) =>
                      changePreset(i, e.target.value as ActionPreset)
                    }
                    aria-label={`Action ${i + 1} type`}
                  >
                    {PRESET_OPTIONS.map((opt) => (
                      <option key={opt.id} value={opt.id}>
                        {opt.label}
                      </option>
                    ))}
                  </select>
                  <div className="flex items-center gap-1 shrink-0">
                    <IconBtn
                      label="Move up"
                      onClick={() => moveAction(i, -1)}
                      disabled={i === 0}
                    >
                      ?
                    </IconBtn>
                    <IconBtn
                      label="Move down"
                      onClick={() => moveAction(i, 1)}
                      disabled={i === actions.length - 1}
                    >
                      ?
                    </IconBtn>
                    <IconBtn
                      label="Remove action"
                      onClick={() => removeAction(i)}
                      danger
                    >
                      ?
                    </IconBtn>
                  </div>
                </div>

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
                {a.type === "shell" && (
                  <FormInput
                    name={`action_shell_${i}`}
                    label={
                      preset === "shell_custom"
                        ? "Command"
                        : "Command (editable)"
                    }
                    value={a.command ?? ""}
                    placeholder={
                      preset === "shell_notify"
                        ? NOTIFY_CMD
                        : preset === "shell_sound"
                          ? SOUND_CMD
                          : "notify-send done $RQBIT_NAME"
                    }
                    help={
                      preset === "shell_custom"
                        ? "Env: RQBIT_TORRENT_ID, RQBIT_INFO_HASH, RQBIT_NAME, RQBIT_OUTPUT_FOLDER."
                        : undefined
                    }
                    onChange={(e) =>
                      updateAction(i, { command: e.target.value })
                    }
                  />
                )}
              </li>
            );
          })}
        </ul>

        <div className="flex items-center gap-2 flex-wrap">
          <select
            className="flex-1 min-w-[12rem] border border-divider rounded bg-surface py-1.5 px-2 text-text text-sm"
            value={addPreset}
            onChange={(e) => setAddPreset(e.target.value as ActionPreset)}
            aria-label="New action type"
          >
            {PRESET_OPTIONS.map((opt) => (
              <option key={opt.id} value={opt.id}>
                {opt.label}
              </option>
            ))}
          </select>
          <IconBtn label="Add action" onClick={addAction}>
            +
          </IconBtn>
          <button
            type="button"
            className="px-3 py-1.5 text-sm border border-divider rounded hover:bg-surface-raised"
            onClick={addAction}
          >
            Add action
          </button>
        </div>
      </Fieldset>

      <CollapsibleSection
        title="Legacy fields"
        summary={
          hasLegacy
            ? "Configured (used when action list is empty)"
            : "Used when action list is empty"
        }
        defaultOpen={hasLegacy && actions.length === 0}
      >
        <FormInput
          name="on_complete_hook"
          label="On-complete hook (shell)"
          value={preferences.on_complete_hook ?? ""}
          placeholder="e.g. notify-send done $RQBIT_NAME"
          help="Shell command run when a torrent finishes. Env: RQBIT_TORRENT_ID, RQBIT_INFO_HASH, RQBIT_NAME, RQBIT_OUTPUT_FOLDER."
          onChange={(e) =>
            onChange({ on_complete_hook: e.target.value || null })
          }
        />
        <FormInput
          name="move_completed_path"
          label="Move completed to"
          value={preferences.move_completed_path ?? ""}
          placeholder="/data/completed"
          help="If set, move (or copy) torrent files here when download finishes. Seeding continues from the new location."
          onChange={(e) =>
            onChange({ move_completed_path: e.target.value || null })
          }
        />
        <FormCheckbox
          checked={!!preferences.move_completed_copy}
          name="move_completed_copy"
          label="Copy instead of move when completing"
          help="Leave originals in place and copy into the completed folder."
          onChange={(e) => onChange({ move_completed_copy: e.target.checked })}
        />
      </CollapsibleSection>
    </div>
  );
};
