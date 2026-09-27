import {
  formatAge,
  parseScanHours,
  relPath,
  selectionSummary,
} from "./cleanup";
let n = 0;
const check = (c: boolean, m: string) => {
  n++;
  if (!c) throw new Error("FAIL: " + m);
};
check(relPath("/dl/Show/a.mkv", "/dl") === "Show/a.mkv", "relative to root");
check(relPath("/dl/Show/a.mkv", "/dl/") === "Show/a.mkv", "root with slash");
check(relPath("/other/a", "/dl") === "/other/a", "outside root unchanged");
check(relPath("D:\\dl\\x.iso", "D:\\dl") === "x.iso", "windows paths");
check(relPath("/dlx/a", "/dl") === "/dlx/a", "prefix but not a child");
check(formatAge(1000 - 59 * 60, 1000) === "59 min ago", "minutes floored");
check(formatAge(1000 - 7199, 1000) === "1 h ago", "hours floored");
check(formatAge(0, 86400 * 3 + 5) === "3 d ago", "days");
check(formatAge(null, 5) === "—", "unknown mtime");
const items = [
  { id: 0, path: "", root: "", kind: "dir" as const, size: 10, mtime: 0, files: 3, reason: "" },
  { id: 1, path: "", root: "", kind: "file" as const, size: 5, mtime: 0, files: 1, reason: "" },
];
const s = selectionSummary(items, new Set([1]));
check(s.count === 1 && s.bytes === 5 && s.files === 1, "selection summary");
check(selectionSummary(items, new Set()).count === 0, "empty selection");
check(parseScanHours("") === null && parseScanHours("0") === null, "empty/0 = off");
check(parseScanHours("24") === 24, "hours");
check(parseScanHours("1.5") === "invalid" && parseScanHours("x") === "invalid", "invalid");
console.log(`cleanup: ${n}/${n} checks passed`);
