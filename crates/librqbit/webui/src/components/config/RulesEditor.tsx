import React from "react";
import {
  RemovePolicy,
  RuleAction,
  SeedingLimits,
  SpeedWindow,
  StalledRule,
} from "../../api-types";
import {
  RULE_ACTION_LABELS,
  SEEDING_ACTIONS,
  STALLED_ACTIONS,
  ruleDeleteWarnings,
} from "../../helper/rules";
import { FormCheckbox } from "../forms/FormCheckbox";
import { UnitInput } from "../forms/UnitInput";

const selectClass =
  "w-full bg-surface border border-divider rounded px-2 py-1.5 text-sm";

const ActionSelect: React.FC<{
  id: string;
  label: string;
  value: RuleAction;
  options: RuleAction[];
  disabled?: boolean;
  onChange: (a: RuleAction) => void;
}> = ({ id, label, value, options, disabled, onChange }) => (
  <div className="flex flex-col gap-1 mb-2">
    <label htmlFor={id} className="text-sm">
      {label}
    </label>
    <select
      id={id}
      className={selectClass}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value as RuleAction)}
    >
      {options.map((a) => (
        <option key={a} value={a}>
          {RULE_ACTION_LABELS[a]}
        </option>
      ))}
    </select>
  </div>
);

export const StalledEditor: React.FC<{
  id: string;
  value: StalledRule;
  onChange: (v: StalledRule) => void;
}> = ({ id, value, onChange }) => (
  <div>
    <FormCheckbox
      name={`${id}-stalled-enabled`}
      checked={value.enabled}
      label="Act on stalled downloads"
      help="Counts only while the torrent is actually running (not paused, queued or checking). Any newly verified piece resets the timer."
      onChange={(e) => onChange({ ...value, enabled: e.target.checked })}
    />
    <div className="grid sm:grid-cols-2 gap-x-4">
      <UnitInput
        kind="duration"
        name={`${id}-stalled-after`}
        label="No verified progress for"
        value={value.after_secs}
        disabled={!value.enabled}
        allowEmpty={false}
        placeholder="e.g. 24h"
        onChange={(v) => v != null && onChange({ ...value, after_secs: v })}
      />
      <ActionSelect
        id={`${id}-stalled-action`}
        label="Then"
        value={value.action}
        options={STALLED_ACTIONS}
        disabled={!value.enabled}
        onChange={(action) => onChange({ ...value, action })}
      />
    </div>
  </div>
);

export const SeedingEditor: React.FC<{
  id: string;
  value: SeedingLimits;
  onChange: (v: SeedingLimits) => void;
}> = ({ id, value, onChange }) => (
  <div>
    <FormCheckbox
      name={`${id}-seeding-enabled`}
      checked={value.enabled}
      label="Seeding limits"
      help="Whichever limit is reached first fires. Empty = no limit of that kind. Seeding time counts only while the torrent is seeding."
      onChange={(e) => onChange({ ...value, enabled: e.target.checked })}
    />
    <div className="grid sm:grid-cols-3 gap-x-4">
      <UnitInput
        kind="duration"
        name={`${id}-seed-time`}
        label="Seeding time"
        value={value.max_seed_secs}
        disabled={!value.enabled}
        placeholder="e.g. 7d"
        onChange={(v) => onChange({ ...value, max_seed_secs: v })}
      />
      <UnitInput
        kind="size"
        name={`${id}-seed-bytes`}
        label="Uploaded"
        value={value.max_uploaded_bytes}
        disabled={!value.enabled}
        placeholder="e.g. 50 GB"
        onChange={(v) => onChange({ ...value, max_uploaded_bytes: v })}
      />
      <div className="flex flex-col gap-1 mb-2">
        <label htmlFor={`${id}-seed-ratio`} className="text-sm">
          Ratio
        </label>
        <input
          id={`${id}-seed-ratio`}
          type="number"
          step="0.1"
          min="0"
          disabled={!value.enabled}
          placeholder="e.g. 2.0"
          className="block border border-divider rounded bg-transparent py-1.5 pl-2 text-sm"
          value={value.max_ratio ?? ""}
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            onChange({
              ...value,
              max_ratio: !e.target.value || isNaN(v) || v <= 0 ? null : v,
            });
          }}
        />
      </div>
    </div>
    <ActionSelect
      id={`${id}-seeding-action`}
      label="Then"
      value={value.action}
      options={SEEDING_ACTIONS}
      disabled={!value.enabled}
      onChange={(action) => onChange({ ...value, action })}
    />
  </div>
);

export const WindowEditor: React.FC<{
  id: string;
  value: SpeedWindow;
  onChange: (v: SpeedWindow) => void;
}> = ({ id, value, onChange }) => (
  <div>
    <FormCheckbox
      name={`${id}-window-enabled`}
      checked={value.enabled}
      label="Full-speed window after completion"
      help="Seed without a per-torrent cap for this long (or until this much is uploaded after completion, whichever first), then cap this torrent's upload rate or stop seeding. The global upload limit still applies."
      onChange={(e) => onChange({ ...value, enabled: e.target.checked })}
    />
    <div className="grid sm:grid-cols-2 gap-x-4">
      <UnitInput
        kind="duration"
        name={`${id}-window-secs`}
        label="Full speed for"
        value={value.full_speed_secs}
        disabled={!value.enabled}
        placeholder="e.g. 2h"
        onChange={(v) => onChange({ ...value, full_speed_secs: v })}
      />
      <UnitInput
        kind="size"
        name={`${id}-window-bytes`}
        label="or until uploaded"
        value={value.full_speed_bytes}
        disabled={!value.enabled}
        placeholder="e.g. 10 GB"
        onChange={(v) => onChange({ ...value, full_speed_bytes: v })}
      />
      <div className="flex flex-col gap-1 mb-2">
        <label htmlFor={`${id}-window-then`} className="text-sm">
          Then
        </label>
        <select
          id={`${id}-window-then`}
          className={selectClass}
          value={value.then}
          disabled={!value.enabled}
          onChange={(e) =>
            onChange({ ...value, then: e.target.value as "cap" | "stop" })
          }
        >
          <option value="cap">Cap upload rate</option>
          <option value="stop">Stop seeding (pause)</option>
        </select>
      </div>
      <div className="flex flex-col gap-1 mb-2">
        <label htmlFor={`${id}-window-cap`} className="text-sm">
          Upload cap (KB/s)
        </label>
        <input
          id={`${id}-window-cap`}
          type="number"
          min="1"
          disabled={!value.enabled || value.then !== "cap"}
          className="block border border-divider rounded bg-transparent py-1.5 pl-2 text-sm"
          value={value.cap_kib_per_sec}
          onChange={(e) => {
            const v = e.target.valueAsNumber;
            if (!isNaN(v) && v >= 1)
              onChange({ ...value, cap_kib_per_sec: Math.floor(v) });
          }}
        />
      </div>
    </div>
  </div>
);

export const RuleWarnings: React.FC<{
  rules: Parameters<typeof ruleDeleteWarnings>[0];
  policy: RemovePolicy;
}> = ({ rules, policy }) => {
  const w = ruleDeleteWarnings(rules, policy);
  if (w.length === 0) return null;
  return (
    <div className="rounded border border-red-400 bg-red-50 dark:bg-red-900/20 p-2 text-sm text-red-700 dark:text-red-300 mb-2">
      <b>These rules can delete files:</b>
      <ul className="list-disc pl-5">
        {w.map((x) => (
          <li key={x}>{x}</li>
        ))}
      </ul>
    </div>
  );
};
