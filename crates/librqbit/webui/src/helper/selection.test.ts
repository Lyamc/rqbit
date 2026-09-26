// Run with `npm test` (bundled by esbuild, executed by node; no test deps).
import {
  SelectionState,
  clearSelected,
  clickSelect,
  emptySelection,
  navigate,
  pruneSelection,
  rangeSelect,
  selectAllIds,
  toggleFocused,
  toggleSelect,
} from "./selection";

let failures = 0;
let checks = 0;
function eq(actual: unknown, expected: unknown, what: string) {
  checks++;
  const norm = (v: unknown) =>
    JSON.stringify(v instanceof Set ? [...v].sort((a, b) => a - b) : v);
  if (norm(actual) !== norm(expected)) {
    failures++;
    console.error(`FAIL ${what}: got ${norm(actual)}, want ${norm(expected)}`);
  }
}
const ids = (s: SelectionState) => s.selected;
const order = [5, 4, 3, 2, 1];

// Clicks.
let s = clickSelect(emptySelection(), 4);
eq(ids(s), new Set([4]), "click");
s = rangeSelect(s, 2, order, false);
eq(ids(s), new Set([4, 3, 2]), "shift+click range");
eq(s.focus, 2, "shift+click focus");
s = rangeSelect(s, 5, order, false);
eq(ids(s), new Set([5, 4]), "shift+click re-spans from same anchor");
s = toggleSelect(s, 1);
eq(ids(s), new Set([5, 4, 1]), "ctrl+click adds");
s = toggleSelect(s, 4);
eq(ids(s), new Set([5, 1]), "ctrl+click removes");
s = rangeSelect(s, 2, order, true);
eq(ids(s), new Set([5, 4, 3, 2, 1]), "ctrl+shift+click adds range");
s = rangeSelect(clearSelected(s), 3, order, false);
eq(ids(s), new Set([3]), "shift+click without anchor = click");
s = rangeSelect(s, 1, [1, 2], false);
eq(ids(s), new Set([1]), "anchor filtered out = click");

// Keyboard.
s = emptySelection();
let r = navigate(s, "down", false, order);
eq(
  [r.index, r.state.focus, r.state.selected.size],
  [0, 5, 0],
  "down moves cursor only",
);
r = navigate(r.state, "down", false, order);
eq([r.state.focus, r.state.selected.size], [4, 0], "down again");
s = toggleFocused(r.state, order);
eq(ids(s), new Set([4]), "space toggles focused");
s = navigate(s, "down", true, order).state;
s = navigate(s, "down", true, order).state;
eq(ids(s), new Set([4, 3, 2]), "shift+down extends");
s = navigate(s, "up", true, order).state;
eq(ids(s), new Set([4, 3]), "shift+up shrinks");
s = navigate(s, "up", true, order).state;
s = navigate(s, "up", true, order).state;
eq([...ids(s)].sort(), [4, 5], "shift+up past anchor flips");
r = navigate(s, "up", false, order);
eq(r.index, 0, "clamped at top");
s = r.state;
for (let i = 0; i < 3; i++) s = navigate(s, "down", false, order).state;
eq([s.focus, s.selected.size], [2, 2], "ctrl/plain down keeps selection");
s = toggleFocused(s, order);
eq(ids(s), new Set([5, 4, 2]), "space adds focused");
s = toggleFocused(s, order);
eq(ids(s), new Set([5, 4]), "space removes focused");
r = navigate(s, "end", true, order);
eq([r.index, [...ids(r.state)]], [4, [2, 1]], "shift+end from anchor");
s = navigate(r.state, "home", true, order).state;
eq(ids(s), new Set([5, 4, 3, 2]), "shift+home from anchor");
s = selectAllIds(navigate(s, "end", false, order).state, order);
eq(ids(s).size, 5, "select all");
s = clearSelected(s);
eq([s.selected.size, s.focus], [0, 1], "esc clears, keeps cursor");
let t = emptySelection();
eq(toggleFocused(t, order), t, "space without cursor");
t = navigate(t, "up", false, order).state;
eq(t.focus, 1, "up without cursor starts at bottom");
eq(toggleFocused(t, [5, 4]), t, "space with hidden cursor");
eq(navigate(t, "down", false, []).index, -1, "empty list");

// Refresh.
s = navigate(clickSelect(emptySelection(), 3), "down", true, [3, 2, 1]).state;
const kept = pruneSelection(s, (id) => id !== 2);
eq([[...ids(kept)], kept.focus, kept.anchor], [[3], null, 3], "prune");
eq(
  pruneSelection(kept, () => true),
  kept,
  "prune no-op keeps identity",
);
eq(
  ids(rangeSelect(kept, 1, [3, 1], false)),
  new Set([3, 1]),
  "anchor survives",
);

console.log(`selection: ${checks - failures}/${checks} checks passed`);
if (failures) {
  throw new Error(`${failures} selection check(s) failed`);
}
