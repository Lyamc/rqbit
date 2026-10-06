// Run with `npm test`.
import {
  planRemove,
  policyDeletesFiles,
  policyFromPrefs,
  incompleteActionText,
} from "./removePrefs";

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
const keep = { complete: "keep", incomplete: "keep" } as const;
const del = { complete: "delete", incomplete: "delete" } as const;
const fwd = { complete: "keep", incomplete: "finish" } as const;

eq(planRemove(null), { confirm: true, policy: keep }, "not loaded: confirm");
eq(planRemove({}), { confirm: true, policy: keep }, "defaults");
eq(
  planRemove({ confirm_remove: false }),
  { confirm: false, policy: keep },
  "no confirm + keep: remove immediately",
);
eq(
  policyFromPrefs({ default_remove_action: "delete_files" }),
  del,
  "legacy delete migrates to delete/delete",
);
eq(
  planRemove({ confirm_remove: false, default_remove_action: "delete_files" })
    .confirm,
  true,
  "deleting files always confirms",
);
eq(
  planRemove({ confirm_remove: false, remove_policy: fwd }, { complete: 3, incomplete: 0 }),
  { confirm: false, policy: fwd },
  "FWD but only complete torrents selected: nothing deleted, no dialog",
);
eq(
  planRemove({ confirm_remove: false, remove_policy: fwd }, { complete: 3, incomplete: 1 })
    .confirm,
  true,
  "FWD with an incomplete torrent deletes partials: dialog",
);
eq(
  planRemove({ confirm_remove: false, remove_policy: fwd }).confirm,
  true,
  "unknown groups: assume files may be deleted",
);
eq(
  policyDeletesFiles({ complete: "delete", incomplete: "keep" }, { complete: 0, incomplete: 2 }),
  false,
  "delete-complete policy with only incomplete torrents keeps files",
);
eq(
  incompleteActionText("finish", 1, 1).includes("Nothing is complete"),
  true,
  "FWD text explains the nothing-complete case",
);
console.log(`removePrefs: ${checks - failures}/${checks} checks passed`);
if (failures) throw new Error(`${failures} removePrefs check(s) failed`);
