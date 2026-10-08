import { useContext, useState } from "react";
import { ErrorDetails, TorrentListItem } from "../../api-types";
import { APIContext } from "../../context";
import { useTorrentStore } from "../../stores/torrentStore";
import {
  CATEGORY_FIELDS,
  CLEAR_CATEGORY,
  CategoryField,
  buildCategoryUpdate,
  editCategoryForm,
  initialCategoryForm,
  validateCategoryField,
} from "../../helper/category";
import { Button } from "../buttons/Button";
import { Modal } from "./Modal";
import { ModalBody } from "./ModalBody";
import { ModalFooter } from "./ModalFooter";

const LABELS: Record<CategoryField, string> = {
  category: "Name",
  category_source: "Source",
  category_id: "Source id",
  torznab_category: "Torznab number",
};

const PLACEHOLDERS: Record<CategoryField, string> = {
  category: "e.g. Anime - English-translated",
  category_source: "e.g. nyaa",
  category_id: "e.g. 1_2",
  torznab_category: "e.g. 5070",
};

/** "Set category…" for one or more torrents. Only edited fields are changed. */
export const CategoryModal: React.FC<{
  torrents: TorrentListItem[];
  onHide: () => void;
}> = ({ torrents, onHide }) => {
  const API = useContext(APIContext);
  const refreshTorrents = useTorrentStore((s) => s.refreshTorrents);
  const [form, setForm] = useState(() => initialCategoryForm(torrents));
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const n = torrents.length;

  const apply = async (clear: boolean) => {
    if (!API.setCategory) return;
    let update;
    try {
      update = clear ? CLEAR_CATEGORY : buildCategoryUpdate(form);
    } catch (e) {
      setError((e as Error).message);
      return;
    }
    if (Object.keys(update).length === 0) {
      onHide();
      return;
    }
    setSaving(true);
    setError(null);
    const errors: string[] = [];
    for (const t of torrents) {
      try {
        await API.setCategory(t.id, update);
      } catch (e) {
        errors.push(
          `${t.name || `id=${t.id}`}: ${(e as ErrorDetails)?.text ?? e}`,
        );
      }
    }
    setSaving(false);
    refreshTorrents();
    if (errors.length) setError(errors.join("\n"));
    else onHide();
  };

  return (
    <Modal
      isOpen
      onClose={onHide}
      title={n > 1 ? `Set category (${n} torrents)` : "Set category"}
    >
      <ModalBody>
        <div
          className="flex flex-col gap-2 text-sm"
          data-testid="category-modal"
        >
          {CATEGORY_FIELDS.map((f) => {
            const s = form[f];
            const fieldError = s.dirty
              ? validateCategoryField(f, s.value)
              : null;
            return (
              <label key={f} className="flex flex-col gap-0.5">
                <span className="text-secondary">{LABELS[f]}</span>
                <input
                  type="text"
                  name={f}
                  value={s.value}
                  placeholder={
                    s.mixed && !s.dirty ? "(mixed, unchanged)" : PLACEHOLDERS[f]
                  }
                  onChange={(e) =>
                    setForm(editCategoryForm(form, f, e.target.value))
                  }
                  className="bg-surface border border-divider rounded px-2 py-1"
                />
                {fieldError && <span className="text-error">{fieldError}</span>}
              </label>
            );
          })}
          <p className="text-tertiary">
            Only edited fields change; an emptied field is cleared. Without a
            Torznab number, auto-organize looks the source and id up. Changes
            affect future organizing only; nothing is moved now.
          </p>
          {error && (
            <pre className="text-error whitespace-pre-wrap">{error}</pre>
          )}
        </div>
      </ModalBody>
      <ModalFooter>
        <Button onClick={() => apply(true)} disabled={saving}>
          Clear category
        </Button>
        <div className="flex-1" />
        <Button variant="cancel" onClick={onHide} disabled={saving}>
          Cancel
        </Button>
        <Button
          variant="primary"
          onClick={() => apply(false)}
          disabled={saving}
        >
          Save
        </Button>
      </ModalFooter>
    </Modal>
  );
};
