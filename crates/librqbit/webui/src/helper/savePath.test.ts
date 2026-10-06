// Run with `npm test`.
import { saveFolderOf } from "./savePath";
import { incompleteActionText } from "./removePrefs";

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

eq(saveFolderOf("/dl/Show S01", "Show S01"), "/dl", "own folder -> parent");
eq(saveFolderOf("/dl/Show S01/", "Show S01"), "/dl", "trailing slash");
eq(saveFolderOf("D:\\dl\\Show", "Show"), "D:\\dl", "windows separators");
eq(saveFolderOf("/dl", "movie.mkv"), "/dl", "single file: folder itself");
eq(saveFolderOf("/done", "Show"), "/done", "loose torrent: folder itself");
eq(saveFolderOf("/Show", "Show"), "/Show", "never cut to empty");
eq(saveFolderOf("/dl/x", null), "/dl/x", "no name");
eq(
  incompleteActionText("finish", 0, 1, true).includes("already moved"),
  true,
  "individual mode: no finish-what's-done wording",
);
eq(
  incompleteActionText("finish", 0, 1, false).includes("run completion actions"),
  true,
  "folder mode: finish-what's-done wording",
);
console.log(`savePath: ${checks - failures}/${checks} checks passed`);
if (failures) throw new Error(`${failures} savePath check(s) failed`);
