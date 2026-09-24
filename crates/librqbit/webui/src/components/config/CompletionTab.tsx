import React, { useState } from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormCheckbox } from "../forms/FormCheckbox";
import { FormInput } from "../forms/FormInput";
import { CompletionAction, SessionPreferences } from "../../api-types";
import { CollapsibleSection } from "./CollapsibleSection";

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

export interface CompletionTabProps {
  preferences: SessionPreferences;
  onChange: (patch: Partial<SessionPreferences>) => void;
}

export const CompletionTab: React.FC<CompletionTabProps> = ({
  preferences,
  onChange,
}) => {
  const actions = preferences.completion_actions || [];
  const [newActionType, setNewActionType] =
    useState<CompletionAction["type"]>("shell");

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
          Ordered actions run when a torrent finishes. If the list is empty,
          rqbit synthesizes actions from the legacy fields below plus incomplete
          extension / auto-organize toggles (drop incomplete ? organize ? move ?
          shell).
        </p>

        {actions.length === 0 && (
          <p className="text-sm italic text-tertiary mb-2">
            No explicit actions ? using legacy / toggle synthesis.
          </p>
        )}

        <ul className="space-y-2 mb-3">
          {actions.map((a, i) => (
            <li
              key={i}
              className="border border-divider rounded p-2 space-y-2 bg-surface"
            >
              <div className="flex items-center gap-2 flex-wrap">
                <span className="text-sm font-mono text-tertiary">
                  {i + 1}.
                </span>
                <span className="text-sm flex-1 min-w-0 truncate">
                  {actionLabel(a)}
                </span>
                <button
                  type="button"
                  className="text-sm px-2 py-0.5 border border-divider rounded hover:bg-surface-raised disabled:opacity-40"
                  onClick={() => moveAction(i, -1)}
                  disabled={i === 0}
                >
                  Up
                </button>
                <button
                  type="button"
                  className="text-sm px-2 py-0.5 border border-divider rounded hover:bg-surface-raised disabled:opacity-40"
                  onClick={() => moveAction(i, 1)}
                  disabled={i === actions.length - 1}
                >
                  Down
                </button>
                <button
                  type="button"
                  className="text-sm px-2 py-0.5 border border-divider rounded text-error hover:bg-surface-raised"
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
            className="border border-divider rounded bg-surface py-1 px-2 text-text"
            value={newActionType}
            onChange={(e) =>
              setNewActionType(e.target.value as CompletionAction["type"])
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
            className="px-3 py-1 border border-divider rounded hover:bg-surface-raised"
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
          onChange={(e) =>
            onChange({ move_completed_copy: e.target.checked })
          }
        />
      </CollapsibleSection>
    </div>
  );
};
