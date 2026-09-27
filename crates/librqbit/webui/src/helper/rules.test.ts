// Run with `npm test`.
import { DEFAULT_RULES, actionMayDelete, ruleDeleteWarnings } from "./rules";

let failures = 0;
let checks = 0;
function eq(actual: unknown, expected: unknown, what: string) {
  checks++;
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    failures++;
    console.error(`FAIL ${what}: got ${JSON.stringify(actual)}, want ${JSON.stringify(expected)}`);
  }
}
const keep = { complete: "keep", incomplete: "keep" } as const;
const delComplete = { complete: "delete", incomplete: "keep" } as const;
const r = DEFAULT_RULES();
eq(r.stalled.enabled || r.seeding.enabled || r.speed_window.enabled, false, "all rules off by default");
eq(ruleDeleteWarnings(r, keep), [], "no warnings by default");
eq(actionMayDelete("pause", false, keep), false, "pause never deletes");
eq(actionMayDelete("remove_finish", false, keep), true, "FWD deletes partials");
eq(actionMayDelete("remove_policy", true, delComplete), true, "policy delete-complete");
eq(actionMayDelete("remove_policy", false, delComplete), false, "policy keeps incomplete");
const risky = {
  ...r,
  stalled: { ...r.stalled, enabled: true, action: "remove_delete" as const },
  seeding: { ...r.seeding, enabled: true, action: "remove_policy" as const },
};
eq(ruleDeleteWarnings(risky, keep).length, 1, "stalled delete warns; seeding with keep policy doesn't");
eq(ruleDeleteWarnings(risky, delComplete).length, 2, "seeding per delete policy warns");
console.log(`rules: ${checks - failures}/${checks} checks passed`);
if (failures) throw new Error(`${failures} rules check(s) failed`);
