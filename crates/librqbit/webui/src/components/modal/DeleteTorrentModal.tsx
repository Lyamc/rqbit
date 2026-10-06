import { useContext, useEffect, useState } from "react";
import {
  CompleteRemoveAction,
  IncompleteRemoveAction,
  RemovePolicy,
  RemovePreview,
  TorrentListItem,
} from "../../api-types";
import { APIContext } from "../../context";
import { ErrorWithLabel } from "../../rqbit-web";
import { useTorrentStore } from "../../stores/torrentStore";
import { useUIStore } from "../../stores/uiStore";
import { usePrefsStore } from "../../stores/prefsStore";
import {
  completeActionText,
  incompleteActionText,
  planRemove,
  policyDeletesFiles,
} from "../../helper/removePrefs";
import { formatBytes } from "../../helper/formatBytes";
import { Button } from "../buttons/Button";
import { ErrorComponent } from "../ErrorComponent";
import { Spinner } from "../Spinner";
import { Modal } from "./Modal";
import { ModalBody } from "./ModalBody";
import { ModalFooter } from "./ModalFooter";

const selectClass =
  "bg-surface border border-divider rounded px-2 py-1 text-sm";

export const DeleteTorrentModal: React.FC<{
  show: boolean;
  onHide: () => void;
  torrents: Pick<TorrentListItem, "id" | "name">[];
}> = ({ show, onHide, torrents }) => {
  const [policy, setPolicy] = useState<RemovePolicy>({
    complete: "keep",
    incomplete: "keep",
  });
  const [preview, setPreview] = useState<RemovePreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ErrorWithLabel | null>(null);
  const [deleting, setDeleting] = useState(false);
  // Removing without a dialog (confirmation off and nothing gets deleted).
  const [silent, setSilent] = useState(false);

  const API = useContext(APIContext);
  const refreshTorrents = useTorrentStore((state) => state.refreshTorrents);
  const clearSelection = useUIStore((state) => state.clearSelection);
  const preferences = usePrefsStore((state) => state.preferences);

  const close = () => {
    setError(null);
    setDeleting(false);
    setSilent(false);
    setPreview(null);
    onHide();
  };

  const removeTorrents = async (p: RemovePolicy) => {
    setDeleting(true);
    setError(null);
    const errors: string[] = [];
    for (const torrent of torrents) {
      try {
        if (API.remove) {
          await API.remove(torrent.id, p);
        } else {
          // Old servers: only keep / delete everything.
          await (policyDeletesFiles(p) ? API.delete : API.forget)(torrent.id);
        }
      } catch (e) {
        const name = torrent.name || `id=${torrent.id}`;
        errors.push(`${name}: ${(e as any)?.text ?? e}`);
      }
    }
    if (errors.length > 0) {
      setError({
        text: `Failed to remove ${errors.length} torrent${errors.length > 1 ? "s" : ""}`,
        details: { text: errors.join("\n") },
      });
      setDeleting(false);
      setSilent(false);
    } else {
      clearSelection();
      refreshTorrents();
      close();
    }
  };

  // Each time the dialog is requested: load which torrents are complete,
  // preset the policy from preferences and, when confirmation is off and
  // nothing would be deleted, remove right away.
  useEffect(() => {
    if (!show || torrents.length === 0) return;
    setError(null);
    setPreview(null);
    const ids = torrents.map((t) => t.id);
    const decide = (pv: RemovePreview | null) => {
      const prefs = pv
        ? { confirm_remove: pv.confirm_remove, remove_policy: pv.policy }
        : preferences;
      const plan = planRemove(
        prefs,
        pv ? { complete: pv.complete, incomplete: pv.incomplete } : undefined,
      );
      setPolicy(plan.policy);
      setPreview(pv);
      if (!plan.confirm) {
        setSilent(true);
        void removeTorrents(plan.policy);
      }
    };
    if (!API.removePreview) {
      decide(null);
      return;
    }
    setLoading(true);
    API.removePreview(ids)
      .then(decide)
      .catch(() => decide(null))
      .finally(() => setLoading(false));
  }, [show]);

  if (!show || torrents.length === 0 || silent) {
    return null;
  }

  const isBulk = torrents.length > 1;
  const title = isBulk ? `Remove ${torrents.length} torrents` : "Remove torrent";
  const byId = new Map(preview?.items.map((i) => [i.id, i]) ?? []);
  const nComplete = preview?.complete ?? 0;
  const nIncomplete = preview?.incomplete ?? 0;
  const known = !!preview;
  const deletes = policyDeletesFiles(
    policy,
    known ? { complete: nComplete, incomplete: nIncomplete } : undefined,
  );
  // "Move files individually as they complete": finished files have already
  // moved, so there is nothing to ask about moving them.
  const individual =
    preview?.files_move_individually ?? preferences?.move_mode === "files";
  const fwd =
    !individual &&
    policy.incomplete === "finish" &&
    (!known || nIncomplete > 0);

  return (
    <Modal isOpen={show} onClose={close} title={title}>
      <ModalBody>
        {loading && (
          <div className="flex justify-center p-2">
            <Spinner />
          </div>
        )}
        <div
          className={`rounded-md bg-gray-50 dark:bg-slate-700/50 p-3 ${
            isBulk ? "max-h-40 overflow-y-auto" : ""
          }`}
        >
          <ul className="space-y-1">
            {torrents.map((torrent) => {
              const it = byId.get(torrent.id);
              return (
                <li
                  key={torrent.id}
                  className="text-gray-800 dark:text-slate-200 flex gap-2 items-baseline"
                  title={torrent.name ?? undefined}
                >
                  <span className="font-medium truncate">
                    {torrent.name || `Torrent #${torrent.id}`}
                  </span>
                  {it && (
                    <span
                      className={`text-xs shrink-0 ${
                        it.complete ? "text-green-600" : "text-amber-600"
                      }`}
                    >
                      {it.complete
                        ? "complete"
                        : `incomplete: ${it.files_complete} done (${formatBytes(
                            it.bytes_complete,
                          )}), ${it.files_partial} unfinished`}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        </div>

        <div className="mt-4 space-y-3 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <span className="w-44 font-medium">
              {known ? `${nComplete} complete` : "Complete torrents"}
            </span>
            <select
              aria-label="Complete torrents"
              className={selectClass}
              value={policy.complete}
              disabled={known && nComplete === 0}
              onChange={(e) =>
                setPolicy({
                  ...policy,
                  complete: e.target.value as CompleteRemoveAction,
                })
              }
            >
              <option value="keep">Keep files</option>
              <option value="delete">Delete files</option>
            </select>
            {(!known || nComplete > 0) && (
              <span className="text-tertiary">
                → {completeActionText(policy.complete)}
              </span>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <span className="w-44 font-medium">
              {known ? `${nIncomplete} incomplete` : "Incomplete torrents"}
            </span>
            <select
              aria-label="Incomplete torrents"
              className={selectClass}
              value={policy.incomplete}
              disabled={known && nIncomplete === 0}
              onChange={(e) =>
                setPolicy({
                  ...policy,
                  incomplete: e.target.value as IncompleteRemoveAction,
                })
              }
            >
              <option value="keep">Keep files</option>
              <option value="delete">Delete all</option>
              <option value="finish">
                {individual
                  ? "Delete unfinished files"
                  : "Finish what's done"}
              </option>
            </select>
            {(!known || nIncomplete > 0) && (
              <span className="text-tertiary">
                →{" "}
                {incompleteActionText(
                  policy.incomplete,
                  preview?.incomplete_nothing_done ?? 0,
                  nIncomplete,
                  individual,
                )}
              </span>
            )}
          </div>
          {fwd && (
            <div className="rounded border border-divider p-2 text-tertiary">
              Completion actions that will run on the finished files:{" "}
              {preview && preview.completion_actions.length > 0 ? (
                <b>{preview.completion_actions.join(" → ")}</b>
              ) : (
                <b>none configured (files stay where they are)</b>
              )}
              . If an action fails the torrent is kept and marked "needs
              attention". Runs in the background; progress shows in the
              torrent's status and in Events.
            </div>
          )}
          {deletes && (
            <p className="text-red-600 dark:text-red-400">
              Files will be deleted from disk. This cannot be undone.
            </p>
          )}
        </div>

        {error && <ErrorComponent error={error} />}
      </ModalBody>

      <ModalFooter>
        {deleting && <Spinner />}
        <Button variant="cancel" onClick={close}>
          Cancel
        </Button>
        <Button
          variant="danger"
          onClick={() => removeTorrents(policy)}
          disabled={deleting || loading}
        >
          {isBulk ? `Remove ${torrents.length} torrents` : "Remove torrent"}
          {deletes ? " (deletes files)" : ""}
        </Button>
      </ModalFooter>
    </Modal>
  );
};
