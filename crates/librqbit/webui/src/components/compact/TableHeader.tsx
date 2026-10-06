import { SortIcon } from "../SortIcon";
import { TableSortColumn } from "./TorrentTable";
import { SortDirection } from "../../helper/torrentFilters";
import { TORRENT_TABLE_CELL_PAD } from "./torrentTableLayout";

interface TableHeaderProps {
  column: TableSortColumn;
  label: string;
  sortColumn: TableSortColumn;
  sortDirection: SortDirection;
  onSort: (column: TableSortColumn) => void;
  className?: string;
  align?: "left" | "center" | "right";
}

export const TableHeader: React.FC<TableHeaderProps> = ({
  column,
  label,
  sortColumn,
  sortDirection,
  onSort,
  className = "",
  align = "left",
}) => {
  const alignClass =
    align === "center"
      ? "text-center"
      : align === "right"
        ? "text-right"
        : "text-left";

  return (
    <div
      role="columnheader"
      className={`${TORRENT_TABLE_CELL_PAD} py-2 text-secondary cursor-pointer hover:text-text select-none whitespace-nowrap ${alignClass} ${className}`}
      onClick={() => onSort(column)}
    >
      {label}
      <SortIcon
        column={column}
        sortColumn={sortColumn}
        sortDirection={sortDirection}
      />
    </div>
  );
};
