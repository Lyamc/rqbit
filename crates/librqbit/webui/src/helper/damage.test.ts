// Run with `npm test`.
import {
  damageShortText,
  diskFullDetail,
  hasRecoveryIssues,
  repairSummaryText,
} from "./damage";
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

const full: DamageStats = {
  damaged_files: [],
  disk_full: {
    since: "2026-10-09T13:15:35Z",
    pieces_waiting: 3,
    space: { free_bytes: 432_100_000, total_bytes: 4e12, mount: "/mnt/data" },
    resume_at_free_bytes: 1 << 30,
    message: "Disk full: downloading paused until space is freed (0.4 GB free on /mnt/data)",
    last_error: "error calling pwritev: ENOSPC: No space left on device",
  },
};
eq(
  damageShortText(full),
  "Disk full: downloading paused until space is freed (0.4 GB free on /mnt/data) — resumes by itself, Fix errors retries now",
  "disk full: plain message in the list",
);
eq(hasRecoveryIssues(full), true, "disk full shows the recovery notice");
eq(
  diskFullDetail(full)?.startsWith("Resumes by itself once 1"),
  true,
  "disk full detail names the resume threshold",
);
eq(diskFullDetail({ damaged_files: [] }), null, "no disk full detail otherwise");

console.log(`damage: ${checks - failures}/${checks} checks passed`);
if (failures) throw new Error(`${failures} damage check(s) failed`);
