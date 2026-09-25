import { useLayoutEffect, useRef, useState } from "react";
import { AddButton, AddButtonGroup } from "./buttons/AddButton";

// @ts-ignore
import Logo from "../../assets/logo.svg?react";

// Extra room required before switching from the single "Add" button back to
// the three split buttons, so resizing around the threshold doesn't flicker.
const EXPAND_HYSTERESIS_PX = 32;
// m-2 on both header children (8px each side) + gap-1 between add & settings.
const LAYOUT_SLACK_PX = 16 + 16 + 4;

const outerWidth = (el: HTMLElement | null) => {
  if (!el) return 0;
  const cs = window.getComputedStyle(el);
  return (
    el.getBoundingClientRect().width +
    (parseFloat(cs.marginLeft) || 0) +
    (parseFloat(cs.marginRight) || 0)
  );
};

/**
 * Show the split Add buttons only when the whole header (title + split
 * buttons + settings) fits on one row; otherwise collapse to the single "Add"
 * button. Widths are measured with ResizeObserver, the split-button width via
 * an invisible, always-rendered copy so it's known even while collapsed.
 */
function useExpandedAdd() {
  const headerRef = useRef<HTMLElement>(null);
  const titleRef = useRef<HTMLDivElement>(null);
  const settingsRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState(false);

  useLayoutEffect(() => {
    const header = headerRef.current;
    if (!header) return;

    const update = () => {
      const available = header.clientWidth;
      const needed =
        outerWidth(titleRef.current) +
        (measureRef.current?.scrollWidth ?? 0) +
        (settingsRef.current?.getBoundingClientRect().width ?? 0) +
        LAYOUT_SLACK_PX;
      setExpanded((prev) =>
        prev ? available >= needed : available >= needed + EXPAND_HYSTERESIS_PX,
      );
    };

    update();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", update);
      return () => window.removeEventListener("resize", update);
    }
    const ro = new ResizeObserver(() => update());
    for (const el of [
      header,
      titleRef.current,
      settingsRef.current,
      measureRef.current,
    ]) {
      if (el) ro.observe(el);
    }
    return () => ro.disconnect();
  }, []);

  return { expanded, headerRef, titleRef, settingsRef, measureRef };
}

export const Header = ({
  title,
  version,
  settingsSlot,
}: {
  title: string;
  version: string;
  settingsSlot?: React.ReactNode;
}) => {
  const { expanded, headerRef, titleRef, settingsRef, measureRef } =
    useExpandedAdd();

  return (
    <header
      ref={headerRef}
      className="relative bg-surface-raised drop-shadow-lg flex flex-wrap justify-center lg:justify-between items-center"
    >
      <div
        ref={titleRef}
        className="flex flex-nowrap items-center justify-between m-2"
      >
        <Logo className="w-10 h-10 p-1" alt="logo" />
        <h1 className="flex items-center">
          <div className="text-3xl font-bold">{title}</div>
          <div className="bg-primary/10 text-primary text-xl font-semibold me-2 px-2.5 py-0.5 rounded ms-2">
            v{version}
          </div>
        </h1>
      </div>
      <div className="flex flex-wrap items-center gap-1 m-2">
        <AddButton expanded={expanded} className="grow justify-center" />
        {settingsSlot && (
          <div ref={settingsRef} className="flex items-center">
            <div className="hidden lg:block w-px h-6 bg-divider mx-2" />
            {settingsSlot}
          </div>
        )}
      </div>
      {/* Invisible copy of the split buttons, used only to measure their width. */}
      <div className="absolute left-0 top-0 invisible pointer-events-none overflow-hidden h-0">
        <AddButtonGroup
          ref={measureRef}
          onOpen={() => {}}
          className="w-max"
          ariaHidden
        />
      </div>
    </header>
  );
};
