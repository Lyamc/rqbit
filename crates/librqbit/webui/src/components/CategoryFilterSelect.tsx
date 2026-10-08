import { useMemo } from "react";
import { useUIStore } from "../stores/uiStore";
import { useTorrentStore } from "../stores/torrentStore";
import {
  CATEGORY_FILTER_ALL,
  CATEGORY_FILTER_NONE,
  categoryFilterOptions,
} from "../helper/category";

/** Category filter (labels of the current torrents). */
export const CategoryFilterSelect: React.FC<{ className?: string }> = ({
  className,
}) => {
  const categoryFilter = useUIStore((s) => s.categoryFilter);
  const setCategoryFilter = useUIStore((s) => s.setCategoryFilter);
  const torrents = useTorrentStore((s) => s.torrents);
  const options = useMemo(() => categoryFilterOptions(torrents), [torrents]);
  // Keep a selected label listed even if no torrent has it any more.
  const shown =
    categoryFilter !== CATEGORY_FILTER_ALL &&
    categoryFilter !== CATEGORY_FILTER_NONE &&
    !options.includes(categoryFilter)
      ? [...options, categoryFilter]
      : options;
  return (
    <select
      value={categoryFilter}
      onChange={(e) => setCategoryFilter(e.target.value)}
      title="Filter by category"
      data-testid="category-filter"
      className={className}
    >
      <option value={CATEGORY_FILTER_ALL}>All categories</option>
      <option value={CATEGORY_FILTER_NONE}>No category</option>
      {shown.map((c) => (
        <option key={c} value={c}>
          {c}
        </option>
      ))}
    </select>
  );
};
