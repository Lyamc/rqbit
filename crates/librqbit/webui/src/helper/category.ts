// Torrent categories: labels, the category filter and the "Set category…" form.

import { CategoryUpdate, TorrentCategoryFields } from "../api-types";

/** Filter values that aren't category labels. */
export const CATEGORY_FILTER_ALL = "__all__";
export const CATEGORY_FILTER_NONE = "__none__";

/** What to show for a torrent's category ("" = none). */
export function categoryLabel(t: TorrentCategoryFields): string {
  if (t.category_label) return t.category_label;
  if (t.category) return t.category;
  if (t.category_source)
    return [t.category_source, t.category_id].filter(Boolean).join(" ");
  return "";
}

/** Distinct category labels, sorted, for the filter. */
export function categoryFilterOptions(
  torrents: TorrentCategoryFields[] | null | undefined,
): string[] {
  const labels = new Set<string>();
  for (const t of torrents ?? []) {
    const l = categoryLabel(t);
    if (l) labels.add(l);
  }
  return [...labels].sort((a, b) =>
    a.localeCompare(b, undefined, { sensitivity: "base" }),
  );
}

export function matchesCategory(
  t: TorrentCategoryFields,
  filter: string,
): boolean {
  if (!filter || filter === CATEGORY_FILTER_ALL) return true;
  const label = categoryLabel(t);
  if (filter === CATEGORY_FILTER_NONE) return label === "";
  return label === filter;
}

/** Sort key: uncategorized last in ascending order. */
export function categorySortValue(t: TorrentCategoryFields): string {
  const l = categoryLabel(t).toLowerCase();
  return l === "" ? "\uffff" : l;
}

export type CategoryField =
  "category" | "category_source" | "category_id" | "torznab_category";

export const CATEGORY_FIELDS: CategoryField[] = [
  "category",
  "category_source",
  "category_id",
  "torznab_category",
];

export interface CategoryFieldState {
  /** Shown in the input ("" when mixed or unset). */
  value: string;
  /** The selected torrents have different values. */
  mixed: boolean;
  /** Edited by the user: sent to the server. */
  dirty: boolean;
}

export type CategoryForm = Record<CategoryField, CategoryFieldState>;

function fieldValue(t: TorrentCategoryFields, f: CategoryField): string {
  const v = t[f];
  return v === undefined || v === null ? "" : String(v);
}

/** Form for one or more torrents: a field is prefilled when they all agree. */
export function initialCategoryForm(
  torrents: TorrentCategoryFields[],
): CategoryForm {
  const form = {} as CategoryForm;
  for (const f of CATEGORY_FIELDS) {
    const values = new Set(torrents.map((t) => fieldValue(t, f)));
    const mixed = values.size > 1;
    form[f] = {
      value: mixed ? "" : ([...values][0] ?? ""),
      mixed,
      dirty: false,
    };
  }
  return form;
}

export function editCategoryForm(
  form: CategoryForm,
  field: CategoryField,
  value: string,
): CategoryForm {
  return { ...form, [field]: { ...form[field], value, dirty: true } };
}

/** Same rules as the server (it checks again). Returns an error or null. */
export function validateCategoryField(
  field: CategoryField,
  raw: string,
): string | null {
  const v = raw.trim();
  if (v === "") return null;
  switch (field) {
    case "category":
      if ([...v].length > 100) return "Category is longer than 100 characters";
      // eslint-disable-next-line no-control-regex
      if (/[\u0000-\u001f\u007f-\u009f]/.test(v))
        return "Category contains control characters";
      return null;
    case "category_source":
      return /^[a-z0-9_-]{1,32}$/.test(v.toLowerCase())
        ? null
        : "Source: 1-32 characters of a-z, 0-9, _ or -";
    case "category_id":
      return /^[A-Za-z0-9_.-]{1,32}$/.test(v)
        ? null
        : "Source id: 1-32 characters of A-Z, a-z, 0-9, _, . or -";
    case "torznab_category":
      return /^\d{1,9}$/.test(v) ? null : "Torznab category must be a number";
  }
}

/**
 * The `POST /torrents/{id}/category` body: only edited fields; an edited empty
 * field clears it (null). Throws on an invalid field.
 */
export function buildCategoryUpdate(form: CategoryForm): CategoryUpdate {
  const update: CategoryUpdate = {};
  for (const f of CATEGORY_FIELDS) {
    const s = form[f];
    if (!s.dirty) continue;
    const err = validateCategoryField(f, s.value);
    if (err) throw new Error(err);
    const v = s.value.trim();
    if (f === "torznab_category") {
      update.torznab_category = v === "" ? null : parseInt(v, 10);
    } else if (f === "category_source") {
      update.category_source = v === "" ? null : v.toLowerCase();
    } else {
      update[f] = v === "" ? null : v;
    }
  }
  return update;
}

/** Clear everything. */
export const CLEAR_CATEGORY: CategoryUpdate = {
  category: null,
  category_source: null,
  category_id: null,
  torznab_category: null,
};

/** Second line of the details row: where the category comes from. */
export function categoryDetail(t: TorrentCategoryFields): string {
  const parts: string[] = [];
  if (t.category_source)
    parts.push([t.category_source, t.category_id].filter(Boolean).join(" "));
  if (t.torznab_category != null) parts.push(`Torznab ${t.torznab_category}`);
  return parts.join(" · ");
}
