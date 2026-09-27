// Progress percentages are always truncated (floored), never rounded:
// 99.9% shows "99%", and "100%" only appears when have === total.

/** Floored percentage of have/total at `decimals` precision, as a number. */
export function floorPercent(
  have: number,
  total: number,
  decimals = 0,
): number {
  if (!(total > 0)) return have >= total ? 100 : 0;
  if (have >= total) return 100;
  if (!(have > 0)) return 0;
  const scale = Math.pow(10, decimals);
  // Multiply before dividing so integer byte counts stay exact.
  let units = Math.floor((have * 100 * scale) / total);
  // Never reach 100 while something is missing.
  units = Math.min(units, 100 * scale - 1);
  return units / scale;
}

/** Floors an already-computed percentage (0..100). Only exactly 100 is 100. */
export function floorPercentValue(pct: number, decimals = 0): number {
  if (!isFinite(pct) || pct <= 0) return 0;
  if (pct >= 100) return 100;
  const scale = Math.pow(10, decimals);
  // Tiny epsilon: 0.29 * 100 is 28.999999999999996 in floating point.
  let units = Math.floor(pct * scale + 1e-9);
  units = Math.min(units, 100 * scale - 1);
  return units / scale;
}

function fmt(units: number, decimals: number): string {
  return units.toFixed(decimals);
}

/** "99", "99.9", "100" — floored, never rounded up. */
export function formatProgress(
  have: number,
  total: number,
  decimals = 0,
): string {
  return fmt(floorPercent(have, total, decimals), decimals);
}

/** Same as formatProgress but for a value that is already a percentage. */
export function formatPercentValue(pct: number, decimals = 0): string {
  return fmt(floorPercentValue(pct, decimals), decimals);
}
