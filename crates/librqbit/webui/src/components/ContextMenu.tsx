import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

export type MenuItem =
  | { separator: true }
  | {
      separator?: false;
      label: string;
      onClick?: () => void;
      disabled?: boolean;
      /** Radio/check mark shown before the label. */
      checked?: boolean;
      danger?: boolean;
      hint?: string;
      submenu?: MenuItem[];
    };

const MenuList: React.FC<{
  items: MenuItem[];
  onClose: () => void;
  style: React.CSSProperties;
  testId?: string;
}> = ({ items, onClose, style, testId }) => {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState(style);
  const [open, setOpen] = useState<number | null>(null);
  const [subPos, setSubPos] = useState<React.CSSProperties>({});

  // Keep the menu on screen.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const next = { ...style };
    const left = Number(style.left ?? 0);
    const top = Number(style.top ?? 0);
    if (left + r.width > window.innerWidth - 4)
      next.left = Math.max(4, window.innerWidth - r.width - 4);
    if (top + r.height > window.innerHeight - 4)
      next.top = Math.max(4, window.innerHeight - r.height - 4);
    setPos(next);
  }, [style.left, style.top]);

  return (
    <div
      ref={ref}
      role="menu"
      data-testid={testId}
      className="fixed z-[1000] min-w-[220px] py-1 rounded-md border border-divider bg-surface-raised shadow-lg text-sm text-text"
      style={pos}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((it, i) =>
        it.separator ? (
          <div key={i} className="my-1 border-t border-divider" />
        ) : (
          <div
            key={i}
            role="menuitem"
            aria-disabled={it.disabled}
            className={`relative flex items-center gap-2 px-3 py-1.5 select-none ${
              it.disabled
                ? "opacity-40 cursor-default"
                : "cursor-pointer hover:bg-primary/15"
            } ${it.danger ? "text-red-600 dark:text-red-400" : ""}`}
            onMouseEnter={(e) => {
              if (it.submenu && !it.disabled) {
                const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                const right = r.right + 230 > window.innerWidth;
                setSubPos({
                  top: r.top - 4,
                  left: right ? r.left - 230 : r.right - 2,
                });
                setOpen(i);
              } else {
                setOpen(null);
              }
            }}
            onClick={(e) => {
              e.stopPropagation();
              if (it.disabled || it.submenu) return;
              onClose();
              it.onClick?.();
            }}
          >
            <span className="w-3 text-center">
              {it.checked ? "✓" : ""}
            </span>
            <span className="flex-1 whitespace-nowrap">{it.label}</span>
            {it.hint && <span className="text-tertiary text-xs">{it.hint}</span>}
            {it.submenu && <span className="text-tertiary">▸</span>}
            {it.submenu && open === i && (
              <MenuList items={it.submenu} onClose={onClose} style={subPos} />
            )}
          </div>
        ),
      )}
    </div>
  );
};

/** Right-click menu at (x, y) (viewport coordinates). */
export const ContextMenu: React.FC<{
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
  testId?: string;
}> = ({ x, y, items, onClose, testId }) => {
  useEffect(() => {
    const close = () => onClose();
    // Capture phase: the table's own key handler stops propagation.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    // Defer so the opening right-click doesn't close it.
    const t = setTimeout(() => {
      window.addEventListener("mousedown", onDown);
      window.addEventListener("blur", close);
      window.addEventListener("resize", close);
      window.addEventListener("keydown", onKey, true);
    }, 0);
    function onDown(e: MouseEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest('[role="menu"]')) return;
      onClose();
    }
    return () => {
      clearTimeout(t);
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [onClose]);
  return createPortal(
    <MenuList
      items={items}
      onClose={onClose}
      style={{ left: x, top: y }}
      testId={testId}
    />,
    document.body,
  );
};
