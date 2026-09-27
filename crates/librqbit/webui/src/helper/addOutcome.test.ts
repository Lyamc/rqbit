import { addOutcome, okLabel } from "./addOutcome";
import { shouldAutoClose } from "./autoClose";
let n = 0;
const check = (c: boolean, m: string) => {
  n++;
  if (!c) throw new Error("FAIL: " + m);
};
check(addOutcome({ resolving: true }) === "resolving", "resolving flag");
check(
  addOutcome({ state: "resolving_metadata" }) === "resolving",
  "resolving state",
);
check(addOutcome({}, "resolving_in_background") === "resolving", "job stage");
check(
  addOutcome({ resolving: true, already_managed: true }) === "already",
  "already wins",
);
check(addOutcome({ state: "added" }) === "added", "plain add");
check(addOutcome(undefined) === "added", "no body");
check(okLabel("resolving") === "added · resolving metadata", "label");
// "Added, resolving" items finish as status "ok": the window auto-closes.
check(
  shouldAutoClose({ total: 2, ok: 2, failed: 0, cancelled: 0 }, ["ok"]),
  "resolving counts as success for auto-close",
);
console.log(`addOutcome: ${n}/${n} checks passed`);
