// Run with `npm test`.
import { formatDuration, parseDuration, parseSize } from "./units";

let failures = 0;
let checks = 0;
function eq(actual: unknown, expected: unknown, what: string) {
  checks++;
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    failures++;
    console.error(`FAIL ${what}: got ${JSON.stringify(actual)}, want ${JSON.stringify(expected)}`);
  }
}
eq(formatDuration(0), "0s", "zero");
eq(formatDuration(7800), "2h 10m", "2h 10m");
eq(formatDuration(90061), "1d 1h", "two most significant units");
eq(parseDuration("15"), 900, "bare = minutes");
eq(parseDuration("90", "s"), 90, "bare seconds");
eq(parseDuration("2h 30m"), 9000, "compound");
eq(parseDuration("1.5h"), 5400, "fractional");
eq(parseDuration("3 days"), 259200, "long unit names");
eq(parseDuration("abc"), null, "invalid");
eq(parseDuration(""), null, "empty");
eq(parseSize("500 MB"), 500 * 1024 ** 2, "MB is binary like the display");
eq(parseSize("1.21 GB"), Math.round(1.21 * 1024 ** 3), "formatBytes output round-trips");
eq(parseSize("0 Bytes"), 0, "Bytes");
eq(parseSize("2GiB"), 2 * 1024 ** 3, "binary");
eq(parseSize("1024"), 1024, "bytes");
eq(parseSize("1.5 x"), null, "invalid size");
console.log(`units: ${checks - failures}/${checks} checks passed`);
if (failures) throw new Error(`${failures} units check(s) failed`);
