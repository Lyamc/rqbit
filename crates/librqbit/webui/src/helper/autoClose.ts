import { BulkImportProgress } from "./bulkImportQueue";

/** The Add window closes by itself only when every item went in (added or already
 *  in rqbit): this run's items judged by the run result, the rest by their status. */
export function shouldAutoClose(
  result: Pick<BulkImportProgress, "total" | "ok" | "failed" | "cancelled">,
  otherStatuses: string[],
): boolean {
  return (
    result.total > 0 &&
    result.ok === result.total &&
    !result.failed &&
    !result.cancelled &&
    otherStatuses.every((s) => s === "ok")
  );
}
