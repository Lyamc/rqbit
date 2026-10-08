// Run with `npm test`.
import {
  CATEGORY_FILTER_ALL,
  CATEGORY_FILTER_NONE,
  CLEAR_CATEGORY,
  buildCategoryUpdate,
  categoryDetail,
  categoryFilterOptions,
  categoryLabel,
  categorySortValue,
  editCategoryForm,
  initialCategoryForm,
  matchesCategory,
  validateCategoryField,
} from "./category";
import { TorrentCategoryFields } from "../api-types";
import { isTorrentVisible } from "./torrentFilters";

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

const anime: TorrentCategoryFields = {
  category_source: "nyaa",
  category_id: "1_2",
  category_label: "Anime - English-translated",
};
const adult: TorrentCategoryFields = {
  torznab_category: 6000,
  category_source: "sukebei",
  category_id: "2_2",
  category_label: "Real Life - Videos",
};
const torznabOnly: TorrentCategoryFields = {
  torznab_category: 5070,
  category_label: "Anime",
};
const none: TorrentCategoryFields = {};

// Labels
eq(categoryLabel(anime), "Anime - English-translated", "label from server");
eq(categoryLabel({ category: "Custom" }), "Custom", "name when no label");
eq(
  categoryLabel({ category_source: "other", category_id: "7" }),
  "other 7",
  "source id fallback",
);
eq(categoryLabel(none), "", "no category");

// Filter
eq(
  categoryFilterOptions([anime, adult, torznabOnly, none, anime]),
  ["Anime", "Anime - English-translated", "Real Life - Videos"],
  "distinct sorted labels",
);
eq(categoryFilterOptions(null), [], "no torrents");
eq(matchesCategory(anime, CATEGORY_FILTER_ALL), true, "all");
eq(matchesCategory(none, CATEGORY_FILTER_NONE), true, "none matches none");
eq(matchesCategory(anime, CATEGORY_FILTER_NONE), false, "none excludes set");
eq(matchesCategory(anime, "Anime"), false, "exact label only");
eq(matchesCategory(torznabOnly, "Anime"), true, "label match");
const t = {
  id: 1,
  info_hash: "",
  name: "Example",
  output_folder: "",
  total_pieces: 0,
  ...adult,
};
eq(isTorrentVisible(t, "", "all"), true, "default category filter is all");
eq(
  isTorrentVisible(t, "", "all", "Real Life - Videos"),
  true,
  "category filter",
);
eq(isTorrentVisible(t, "", "all", "Anime"), false, "category filter excludes");
eq(
  isTorrentVisible(t, "nothing", "all", "Real Life - Videos"),
  false,
  "search still applies",
);

// Sort: uncategorized last ascending
const sorted = [none, torznabOnly, adult].sort((a, b) =>
  categorySortValue(a).localeCompare(categorySortValue(b)),
);
eq(sorted.map(categoryLabel), ["Anime", "Real Life - Videos", ""], "sort");

// Details row
eq(categoryDetail(adult), "sukebei 2_2 · Torznab 6000", "detail");
eq(categoryDetail(anime), "nyaa 1_2", "detail without torznab");
eq(categoryDetail(none), "", "detail none");

// Form: shared values prefilled, mixed ones empty
let form = initialCategoryForm([anime, { ...anime, category_id: "1_4" }]);
eq(
  form.category_source,
  { value: "nyaa", mixed: false, dirty: false },
  "shared value",
);
eq(form.category_id, { value: "", mixed: true, dirty: false }, "mixed value");
eq(buildCategoryUpdate(form), {}, "nothing edited: empty update");
form = editCategoryForm(form, "category", "  Anime - Raw ");
form = editCategoryForm(form, "torznab_category", "5070");
eq(
  buildCategoryUpdate(form),
  { category: "Anime - Raw", torznab_category: 5070 },
  "only edited fields",
);
form = editCategoryForm(form, "category_source", "");
eq(buildCategoryUpdate(form).category_source, null, "emptied field clears");
form = editCategoryForm(form, "category_source", "NYAA");
eq(buildCategoryUpdate(form).category_source, "nyaa", "source lowercased");
form = editCategoryForm(form, "torznab_category", "Anime");
let threw = false;
try {
  buildCategoryUpdate(form);
} catch {
  threw = true;
}
eq(threw, true, "non-numeric torznab rejected");
eq(
  CLEAR_CATEGORY,
  {
    category: null,
    category_source: null,
    category_id: null,
    torznab_category: null,
  },
  "clear",
);

// Validation mirrors the server
eq(validateCategoryField("category", "x".repeat(100)), null, "100 chars ok");
eq(
  validateCategoryField("category", "x".repeat(101)) !== null,
  true,
  "101 chars rejected",
);
eq(
  validateCategoryField("category", "a\nb") !== null,
  true,
  "control chars rejected",
);
eq(
  validateCategoryField("category_source", "bad source") !== null,
  true,
  "bad source",
);
eq(validateCategoryField("category_id", "1/2") !== null, true, "bad id");
eq(validateCategoryField("category_id", "1_2"), null, "good id");
eq(validateCategoryField("torznab_category", ""), null, "empty ok");

console.log(`category: ${checks - failures}/${checks} checks passed`);
if (failures) throw new Error(`${failures} category check(s) failed`);
