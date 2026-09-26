/**
 * File-manager style multi-selection over the displayed (sorted/filtered)
 * torrent list, keyed by torrent id so it survives polling. Pure functions;
 * the GPUI client implements the same semantics (list_state.rs `Selection`).
 *
 * - click: only that row; anchor + focus move there;
 * - shift+click: anchor..row replaces the selection (anchor kept);
 *   ctrl+shift+click adds the range;
 * - ctrl/cmd+click, Space: toggle the row; anchor + focus move there;
 * - arrows / Home / End (with or without ctrl): move the focus only;
 *   with shift: anchor..focus replaces the selection.
 */
export interface SelectionState {
  selected: Set<number>;
  anchor: number | null;
  /** Keyboard cursor (focus ring). */
  focus: number | null;
}

export type Nav = "up" | "down" | "home" | "end";

export const emptySelection = (): SelectionState => ({
  selected: new Set(),
  anchor: null,
  focus: null,
});

export function clickSelect(_s: SelectionState, id: number): SelectionState {
  return { selected: new Set([id]), anchor: id, focus: id };
}

export function toggleSelect(s: SelectionState, id: number): SelectionState {
  const selected = new Set(s.selected);
  if (!selected.delete(id)) selected.add(id);
  return { selected, anchor: id, focus: id };
}

export function rangeSelect(
  s: SelectionState,
  id: number,
  ordered: number[],
  additive: boolean,
): SelectionState {
  const a = s.anchor === null ? -1 : ordered.indexOf(s.anchor);
  const b = ordered.indexOf(id);
  if (a === -1 || b === -1) {
    return additive ? toggleSelect(s, id) : clickSelect(s, id);
  }
  const selected = additive ? new Set(s.selected) : new Set<number>();
  for (const x of ordered.slice(Math.min(a, b), Math.max(a, b) + 1)) {
    selected.add(x);
  }
  return { selected, anchor: s.anchor, focus: id };
}

/** Returns the new state and the focus' index in `ordered` (-1 if none). */
export function navigate(
  s: SelectionState,
  nav: Nav,
  extend: boolean,
  ordered: number[],
): { state: SelectionState; index: number } {
  if (ordered.length === 0) return { state: s, index: -1 };
  const last = ordered.length - 1;
  let current = s.focus === null ? -1 : ordered.indexOf(s.focus);
  if (current === -1) current = ordered.findIndex((x) => s.selected.has(x));
  let target: number;
  switch (nav) {
    case "home":
      target = 0;
      break;
    case "end":
      target = last;
      break;
    case "up":
      target = current === -1 ? last : Math.max(current - 1, 0);
      break;
    case "down":
      target = current === -1 ? 0 : Math.min(current + 1, last);
      break;
  }
  const focus = ordered[target];
  if (!extend) {
    return { state: { ...s, focus }, index: target };
  }
  let anchor =
    s.anchor !== null && ordered.includes(s.anchor)
      ? s.anchor
      : current !== -1
        ? ordered[current]
        : focus;
  const a = ordered.indexOf(anchor);
  const selected = new Set(
    ordered.slice(Math.min(a, target), Math.max(a, target) + 1),
  );
  return { state: { selected, anchor, focus }, index: target };
}

/** Space: toggles the focused row if it is displayed. */
export function toggleFocused(
  s: SelectionState,
  ordered: number[],
): SelectionState {
  if (s.focus === null || !ordered.includes(s.focus)) return s;
  return toggleSelect(s, s.focus);
}

export function selectAllIds(
  s: SelectionState,
  ordered: number[],
): SelectionState {
  return { ...s, selected: new Set(ordered) };
}

/** Esc. The focus stays so the keyboard cursor doesn't jump. */
export function clearSelected(s: SelectionState): SelectionState {
  return { selected: new Set(), anchor: null, focus: s.focus };
}

/** Drops ids that no longer exist; returns `s` itself if nothing changed. */
export function pruneSelection(
  s: SelectionState,
  exists: (id: number) => boolean,
): SelectionState {
  const gone = [...s.selected].some((id) => !exists(id));
  const anchorGone = s.anchor !== null && !exists(s.anchor);
  const focusGone = s.focus !== null && !exists(s.focus);
  if (!gone && !anchorGone && !focusGone) return s;
  return {
    selected: gone
      ? new Set([...s.selected].filter((id) => exists(id)))
      : s.selected,
    anchor: anchorGone ? null : s.anchor,
    focus: focusGone ? null : s.focus,
  };
}
