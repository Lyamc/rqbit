export type BulkItemStatus = "pending" | "running" | "ok" | "error" | "cancelled";

export type BulkImportItem = {
  id: string;
  label: string;
  status: BulkItemStatus;
  error?: string;
};

export type BulkImportProgress = {
  total: number;
  done: number;
  ok: number;
  failed: number;
  cancelled: number;
  running: number;
  items: BulkImportItem[];
};

export type BulkWorkItem<T> = {
  id: string;
  label: string;
  data: T;
};

/**
 * Run work items with limited concurrency. Per-item failures do not abort the batch.
 * Set signal.cancelled = true to stop scheduling new work; in-flight items still finish.
 */
export async function runBulkQueue<T>(opts: {
  items: BulkWorkItem<T>[];
  concurrency: number;
  worker: (data: T, item: BulkWorkItem<T>) => Promise<void>;
  onProgress?: (progress: BulkImportProgress) => void;
  signal?: { cancelled: boolean };
}): Promise<BulkImportProgress> {
  const { items, worker, onProgress, signal } = opts;
  const concurrency = Math.max(1, Math.min(8, (opts.concurrency | 0) || 4));

  const state: BulkImportItem[] = items.map((i) => ({
    id: i.id,
    label: i.label,
    status: "pending",
  }));

  let ok = 0;
  let failed = 0;
  let cancelled = 0;
  let running = 0;
  let cursor = 0;

  const emit = () => {
    const done = ok + failed + cancelled;
    onProgress?.({
      total: items.length,
      done,
      ok,
      failed,
      cancelled,
      running,
      items: state.map((s) => ({ ...s })),
    });
  };

  emit();

  const claimNext = (): number | null => {
    if (cursor >= items.length) return null;
    return cursor++;
  };

  const runOne = async (index: number) => {
    const work = items[index];
    const st = state[index];

    if (signal?.cancelled) {
      st.status = "cancelled";
      cancelled++;
      emit();
      return;
    }

    st.status = "running";
    running++;
    emit();
    try {
      await worker(work.data, work);
      st.status = "ok";
      ok++;
    } catch (e: unknown) {
      st.status = "error";
      const err = e as { text?: string; message?: string };
      st.error =
        typeof err?.text === "string"
          ? err.text
          : err?.message
            ? String(err.message)
            : String(e);
      failed++;
    } finally {
      running--;
      emit();
    }
  };

  const workers: Promise<void>[] = [];
  for (let w = 0; w < concurrency; w++) {
    workers.push(
      (async () => {
        while (true) {
          const i = claimNext();
          if (i === null) return;
          await runOne(i);
        }
      })(),
    );
  }

  await Promise.all(workers);
  emit();

  return {
    total: items.length,
    done: ok + failed + cancelled,
    ok,
    failed,
    cancelled,
    running: 0,
    items: state,
  };
}
