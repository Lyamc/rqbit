// Run with `npm test`.
import { damageShortText, repairSummaryText } from "./damage";
import { DamageStats, RepairSummary } from "../api-types";

let failures = 0;
let checks = 0;
function eq(actual: unknown, expected: unknown, what: string) {
  checks++;
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    failures++;
    console.error(
      `FAIL ${what}: got ${JSON.stringify(actual)}, want ${JSON.stringify(expected)}`,
    );
  }
}

const nothing: RepairSummary = {
  files_scanned: 3,
  files_repaired: 0,
  files_failed: 0,
  bytes_unreadable: 0,
  bytes_zeroed: 0,
  pieces_to_redownload: 0,
  pieces_invalidated: 0,
  files: [],
};
const fixed: RepairSummary = {
  ...nothing,
  files_scanned: 1,
  files_repaired: 1,
  bytes_unreadable: 4096,
  bytes_zeroed: 4096,
  pieces_to_redownload: 1,
  files: [
    {
      file_id: 0,
      path: "a.mkv",
      bytes_total: 1 << 20,
      bytes_unreadable: 4096,
      bytes_zeroed: 4096,
      ranges_zeroed: [[0, 4096]],
      pieces: [0],
      method: "punch_hole",
    },
  ],
};
const done = (summary: RepairSummary): DamageStats => ({
  damaged_files: [],
  repair: {
    state: "done",
    auto: true,
    started_at: "2026-10-05T00:00:00Z",
    scanned_bytes: 0,
    total_bytes: 0,
    files_total: summary.files_scanned,
    files_done: summary.files_scanned,
    summary,
  },
});

eq(damageShortText(done(nothing)), "No damage found", "nothing found: not 'Repaired'");
eq(
  repairSummaryText(nothing),
  "No damage found (3 file(s) checked).",
  "nothing found summary",
);
eq(
  damageShortText(done(fixed))?.startsWith("Repaired: "),
  true,
  "real repair still says Repaired",
);
eq(damageShortText({ damaged_files: [] }), null, "no damage, no repair: nothing shown");

console.log(`damage: ${checks - failures}/${checks} checks passed`);
if (failures) throw new Error(`${failures} damage check(s) failed`);
