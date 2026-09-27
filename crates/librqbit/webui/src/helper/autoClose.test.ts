import { shouldAutoClose } from "./autoClose";
let n = 0;
const check = (c: boolean, m: string) => {
  n++;
  if (!c) throw new Error("FAIL: " + m);
};
const r = (total: number, ok: number, failed = 0, cancelled = 0) => ({
  total,
  ok,
  failed,
  cancelled,
});
check(shouldAutoClose(r(3, 3), []), "all ok closes");
check(shouldAutoClose(r(2, 2), ["ok"]), "earlier ok items don't block");
check(!shouldAutoClose(r(3, 2, 1), []), "an error keeps it open");
check(!shouldAutoClose(r(3, 2, 0, 1), []), "a cancel keeps it open");
check(
  !shouldAutoClose(r(2, 2), ["error"]),
  "an earlier error still in the queue keeps it open",
);
check(
  !shouldAutoClose(r(2, 2), ["ready"]),
  "unstarted (e.g. unmatched transfer) items keep it open",
);
check(!shouldAutoClose(r(0, 0), []), "nothing ran: stay open");
console.log(`autoClose: ${n}/${n} checks passed`);
