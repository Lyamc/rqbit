import { useContext, useEffect, useState } from "react";
import {
  DownloadOrderPatch,
  DownloadOrderView,
  ErrorDetails,
  FileOrder,
  RulesOverride,
  TorrentRules,
  TorrentRulesView,
} from "../../api-types";
import { APIContext } from "../../context";
import { usePrefsStore } from "../../stores/prefsStore";
import { policyFromPrefs } from "../../helper/removePrefs";
import { formatBytes } from "../../helper/formatBytes";
import { formatDuration } from "../../helper/units";
import { FILE_ORDER_NAMES } from "../../helper/downloadOrderMenu";
import {
  RuleWarnings,
  SeedingEditor,
  StalledEditor,
  WindowEditor,
} from "../config/RulesEditor";

type Key = keyof TorrentRules;
const errText = (e: unknown): string => {
  const t = (e as ErrorDetails)?.text;
  return typeof t === "string" && t ? t : String(e);
};
const LABELS: Record<Key, string> = {
  stalled: "Stalled / no progress",
  seeding: "Seeding limits",
  speed_window: "Full-speed window",
};

const triValue = (v: boolean | undefined) =>
  v === undefined ? "" : v ? "on" : "off";
const triParse = (s: string): boolean | null =>
  s === "" ? null : s === "on";

/** Details → Rules: automatic-rule counters/status, per-torrent overrides and download order. */
export const RulesTab: React.FC<{ torrentId: number }> = ({ torrentId }) => {
  const API = useContext(APIContext);
  const prefs = usePrefsStore((s) => s.preferences);
  const [view, setView] = useState<TorrentRulesView | null>(null);
  const [draft, setDraft] = useState<RulesOverride>({});
  const [dirty, setDirty] = useState(false);
  const [order, setOrder] = useState<DownloadOrderView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = (resetDraft: boolean) => {
    API.getTorrentRules?.(torrentId).then(
      (v) => {
        setView(v);
        if (resetDraft) setDraft(v.override ?? {});
      },
      (e: ErrorDetails) => setError(errText(e)),
    );
  };
  useEffect(() => {
    setDirty(false);
    load(true);
    API.getDownloadOrder?.(torrentId).then(setOrder, () => setOrder(null));
    const t = setInterval(() => load(false), 5000);
    return () => clearInterval(t);
  }, [torrentId]);

  if (!API.getTorrentRules) {
    return <div className="p-4 text-tertiary">Not supported by this server.</div>;
  }
  if (!view) {
    return <div className="p-4 text-tertiary">{error ?? "Loading..."}</div>;
  }

  const save = async (o: RulesOverride | null) => {
    try {
      const v = await API.setTorrentRules!(torrentId, o);
      setView(v);
      setDraft(v.override ?? {});
      setDirty(false);
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  };

  const applyOrder = async (patch: DownloadOrderPatch) => {
    try {
      setOrder(await API.setDownloadOrder!(torrentId, patch));
    } catch (e) {
      setError(errText(e));
    }
  };

  const effectiveDraft: TorrentRules = {
    stalled: draft.stalled ?? view.global.stalled,
    seeding: draft.seeding ?? view.global.seeding,
    speed_window: draft.speed_window ?? view.global.speed_window,
  };
  const set = (k: Key, v: any) => {
    setDraft({ ...draft, [k]: v });
    setDirty(true);
  };
  const c = view.counters;

  return (
    <div className="p-3 text-sm space-y-3">
      <div className="rounded border border-divider p-2">
        <div className="font-medium mb-1">Status</div>
        {view.status.length === 0 ? (
          <div className="text-tertiary">No automatic rule applies.</div>
        ) : (
          <ul className="list-disc pl-5" data-testid="rules-status">
            {view.status.map((s) => (
              <li key={s}>{s}</li>
            ))}
          </ul>
        )}
        <div className="text-tertiary mt-1">
          Seeding time {formatDuration(c.seeding_secs)} · uploaded{" "}
          {formatBytes(c.uploaded_total)} · ratio {view.ratio.toFixed(2)} · no
          progress for {formatDuration(c.idle_secs)}
        </div>
      </div>

      <RuleWarnings rules={effectiveDraft} policy={policyFromPrefs(prefs)} />

      {(Object.keys(LABELS) as Key[]).map((k) => {
        const overridden = draft[k] != null;
        return (
          <div key={k} className="rounded border border-divider p-2">
            <label className="flex items-center gap-2 font-medium">
              <input
                type="checkbox"
                checked={overridden}
                onChange={(e) =>
                  set(k, e.target.checked ? { ...view.global[k] } : null)
                }
              />
              {LABELS[k]}: {overridden ? "custom for this torrent" : "global default"}
            </label>
            {overridden && (
              <div className="mt-2">
                {k === "stalled" && (
                  <StalledEditor id={`t${torrentId}`} value={draft.stalled!} onChange={(v) => set(k, v)} />
                )}
                {k === "seeding" && (
                  <SeedingEditor id={`t${torrentId}`} value={draft.seeding!} onChange={(v) => set(k, v)} />
                )}
                {k === "speed_window" && (
                  <WindowEditor id={`t${torrentId}`} value={draft.speed_window!} onChange={(v) => set(k, v)} />
                )}
              </div>
            )}
          </div>
        );
      })}
      <div className="flex gap-2">
        <button
          type="button"
          disabled={!dirty}
          className="px-3 py-1 rounded bg-primary text-white disabled:opacity-40"
          onClick={() => save(draft)}
        >
          Save rules
        </button>
        <button
          type="button"
          className="px-3 py-1 rounded border border-divider"
          onClick={() => save(null)}
        >
          Use global defaults
        </button>
        {error && <span className="text-red-600">{error}</span>}
      </div>

      {order && API.setDownloadOrder && (
        <div className="rounded border border-divider p-2" data-testid="download-order">
          <div className="font-medium mb-1">Download order</div>
          <div className="text-tertiary mb-2">Current: {order.summary}</div>
          <div className="grid sm:grid-cols-2 gap-2">
            <label className="flex flex-col gap-1">
              Sequential file download
              <select
                className="bg-surface border border-divider rounded px-2 py-1"
                value={triValue(order.torrent.sequential_files)}
                onChange={(e) => applyOrder({ sequential_files: triParse(e.target.value) })}
              >
                <option value="">Default ({order.global.sequential_files ? "on" : "off"})</option>
                <option value="on">On</option>
                <option value="off">Off</option>
              </select>
            </label>
            <label className="flex flex-col gap-1">
              File order
              <select
                className="bg-surface border border-divider rounded px-2 py-1"
                value={order.torrent.file_order ?? ""}
                onChange={(e) =>
                  applyOrder({
                    file_order: e.target.value ? (e.target.value as FileOrder) : null,
                  })
                }
              >
                <option value="">
                  Default ({FILE_ORDER_NAMES[order.global.file_order].toLowerCase()})
                </option>
                {(Object.keys(FILE_ORDER_NAMES) as FileOrder[]).map((k) => (
                  <option key={k} value={k}>
                    {FILE_ORDER_NAMES[k]}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1">
              Sequential download (all files)
              <select
                className="bg-surface border border-divider rounded px-2 py-1"
                value={triValue(order.torrent.sequential)}
                onChange={(e) => applyOrder({ sequential: triParse(e.target.value) })}
              >
                <option value="">Default ({order.global.sequential ? "on" : "off"})</option>
                <option value="on">On</option>
                <option value="off">Off</option>
              </select>
            </label>
            <label className="flex flex-col gap-1">
              First and last pieces first (all files)
              <select
                className="bg-surface border border-divider rounded px-2 py-1"
                value={triValue(order.torrent.first_last_first)}
                onChange={(e) => applyOrder({ first_last_first: triParse(e.target.value) })}
              >
                <option value="">Default ({order.global.first_last_first ? "on" : "off"})</option>
                <option value="on">On</option>
                <option value="off">Off</option>
              </select>
            </label>
          </div>
          <div className="text-tertiary mt-1">
            Per-file settings (Files tab, right-click) override these.
          </div>
        </div>
      )}
    </div>
  );
};
