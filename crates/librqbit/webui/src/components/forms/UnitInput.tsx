import React, { useEffect, useState } from "react";
import { formatDuration, parseDuration, parseSize } from "../../helper/units";
import { formatBytes } from "../../helper/formatBytes";

/**
 * Text input for a duration ("2h 30m") or a size ("1.5 GB"). Keeps the typed
 * text while editing and commits the parsed value (null when empty) on every
 * valid change; invalid text is flagged and not committed.
 */
export const UnitInput: React.FC<{
  kind: "duration" | "size";
  name: string;
  label: string;
  value: number | null | undefined;
  onChange: (v: number | null) => void;
  help?: string;
  placeholder?: string;
  disabled?: boolean;
  bareUnit?: "s" | "m" | "h";
  allowEmpty?: boolean;
}> = ({
  kind,
  name,
  label,
  value,
  onChange,
  help,
  placeholder,
  disabled,
  bareUnit,
  allowEmpty = true,
}) => {
  const fmt = (v: number | null | undefined) =>
    v == null ? "" : kind === "duration" ? formatDuration(v) : formatBytes(v);
  const [text, setText] = useState(fmt(value));
  const [invalid, setInvalid] = useState(false);
  useEffect(() => {
    // External change (e.g. reset): resync unless the text already means it.
    const parsed =
      kind === "duration" ? parseDuration(text, bareUnit) : parseSize(text);
    if (parsed !== (value ?? null)) setText(fmt(value));
  }, [value]);
  return (
    <div className="flex flex-col gap-1 mb-2">
      <label htmlFor={name} className="text-sm">
        {label}
      </label>
      <input
        id={name}
        name={name}
        disabled={disabled}
        placeholder={placeholder}
        className={`block border rounded bg-transparent py-1.5 pl-2 text-sm focus:ring-0 ${
          invalid ? "border-red-500" : "border-divider focus:border-primary"
        }`}
        value={text}
        onChange={(e) => {
          const t = e.target.value;
          setText(t);
          if (!t.trim()) {
            setInvalid(!allowEmpty);
            if (allowEmpty) onChange(null);
            return;
          }
          const v =
            kind === "duration" ? parseDuration(t, bareUnit) : parseSize(t);
          setInvalid(v == null);
          if (v != null) onChange(v);
        }}
        onBlur={() => {
          if (!invalid) setText(fmt(value));
        }}
      />
      {help && <div className="text-sm text-secondary">{help}</div>}
    </div>
  );
};
