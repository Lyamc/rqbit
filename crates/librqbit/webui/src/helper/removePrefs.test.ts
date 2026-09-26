// Run with `npm test`.
import { planRemove } from "./removePrefs";

let failures = 0;
function eq(actual: unknown, expected: unknown, what: string) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    failures++;
    console.error(
      `FAIL ${what}: got ${JSON.stringify(actual)}, want ${JSON.stringify(expected)}`,
    );
  }
}
const both = (confirm: boolean, deleteFiles: boolean) => ({
  confirm,
  deleteFiles,
});
eq(planRemove(null), both(true, false), "not loaded: confirm, keep files");
eq(planRemove({}), both(true, false), "defaults");
eq(
  planRemove({ confirm_remove: false }),
  both(false, false),
  "no confirm: remove immediately, keep files",
);
eq(
  planRemove({ confirm_remove: true, default_remove_action: "delete_files" }),
  both(true, true),
  "delete default presets the checkbox",
);
eq(
  planRemove({ confirm_remove: false, default_remove_action: "delete_files" }),
  both(true, true),
  "deleting files always confirms",
);
console.log(`removePrefs: ${5 - failures}/5 checks passed`);
if (failures) throw new Error(`${failures} removePrefs check(s) failed`);
