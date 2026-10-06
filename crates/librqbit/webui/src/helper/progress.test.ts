import {
  floorPercent,
  floorPercentValue,
  formatPercentValue,
  formatProgress,
} from "./progress";
let n = 0;
const check = (c: boolean, m: string) => {
  n++;
  if (!c) throw new Error("FAIL: " + m);
};
const eq = (a: string, b: string, m: string) => check(a === b, `${m}: ${a} !== ${b}`);
eq(formatProgress(999, 1000), "99", "99.9% -> 99");
eq(formatProgress(9999, 10000, 1), "99.9", "99.99% at 1 decimal -> 99.9");
eq(formatProgress(99999, 100000, 2), "99.99", "99.999% at 2 decimals -> 99.99");
eq(formatProgress(1000, 1000), "100", "complete -> 100");
eq(formatProgress(1000, 1000, 1), "100.0", "complete at 1 decimal -> 100.0");
eq(formatProgress(0, 1000), "0", "nothing -> 0");
eq(formatProgress(1, 1000), "0", "0.1% -> 0");
eq(formatProgress(29, 100), "29", "0.29 is 29 (float trap)");
eq(formatProgress(57, 100, 1), "57.0", "0.57 -> 57.0 (float trap)");
eq(formatProgress(1, 3, 2), "33.33", "1/3 -> 33.33");
eq(formatProgress(2, 3), "66", "2/3 -> 66 (not 67)");
const big = 3 * 1024 ** 4; // 3 TiB
eq(formatProgress(big - 1, big), "99", "1 byte short of 3 TiB -> 99");
eq(formatProgress(big - 1, big, 2), "99.99", "1 byte short at 2 decimals -> 99.99");
check(floorPercent(big - 1, big, 2) < 100, "never 100 when have < total");
eq(formatProgress(0, 0), "100", "zero-length torrent counts as complete");
eq(formatPercentValue(99.99, 1), "99.9", "value 99.99 -> 99.9");
eq(formatPercentValue(99.9999999, 2), "99.99", "value just under 100 -> 99.99");
eq(formatPercentValue(100), "100", "value 100 -> 100");
eq(formatPercentValue(28.99), "28", "value 28.99 -> 28");
eq(formatPercentValue((29 / 100) * 100), "29", "value 0.29*100 -> 29");
check(floorPercentValue(-5) === 0 && floorPercentValue(NaN) === 0, "junk -> 0");
console.log(`progress: ${n}/${n} checks passed`);
