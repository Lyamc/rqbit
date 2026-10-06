import React, { useState, type ReactNode } from "react";

export interface CollapsibleSectionProps {
  title: string;
  /** Short hint shown next to the title when collapsed */
  summary?: string;
  defaultOpen?: boolean;
  children: ReactNode;
  className?: string;
}

/** Progressive-disclosure block matching webui borders/typography. */
export const CollapsibleSection: React.FC<CollapsibleSectionProps> = ({
  title,
  summary,
  defaultOpen = false,
  children,
  className = "",
}) => {
  const [open, setOpen] = useState(defaultOpen);

  return (
    <div
      className={`border border-divider rounded overflow-hidden ${className}`}
    >
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-2 px-3 py-2 text-left bg-surface-raised hover:bg-surface-sunken transition-colors cursor-pointer"
        aria-expanded={open}
      >
        <span
          className={`text-tertiary text-sm transition-transform ${open ? "rotate-90" : ""}`}
          aria-hidden
        >
          ?
        </span>
        <span className="font-medium text-text flex-1">{title}</span>
        {!open && summary && (
          <span className="text-sm text-tertiary truncate max-w-[50%]">
            {summary}
          </span>
        )}
      </button>
      {open && (
        <div className="px-3 py-3 space-y-3 border-t border-divider">
          {children}
        </div>
      )}
    </div>
  );
};
