import React from "react";
import { Fieldset } from "../forms/Fieldset";
import { FormCheckbox } from "../forms/FormCheckbox";
import { FormInput } from "../forms/FormInput";
import { AutoOrganizeFolders, SessionPreferences } from "../../api-types";
import { CollapsibleSection } from "./CollapsibleSection";

export const DEFAULT_ORGANIZE_FOLDERS: AutoOrganizeFolders = {
  anime: "Anime",
  tv: "TV",
  movie: "Movies",
  game: "Games",
  porn: "Adult",
  music: "Music",
  book: "Books",
  software: "Software",
  other: "Other",
};

export interface OrganizeTabProps {
  preferences: SessionPreferences;
  onChange: (patch: Partial<SessionPreferences>) => void;
}

export const OrganizeTab: React.FC<OrganizeTabProps> = ({
  preferences,
  onChange,
}) => {
  const folderKeys = Object.keys(
    DEFAULT_ORGANIZE_FOLDERS,
  ) as (keyof AutoOrganizeFolders)[];
  const enabled = !!preferences.auto_organize_enabled;

  return (
    <div className="text-secondary py-2 space-y-4">
      <Fieldset label="Auto-organize">
        <FormCheckbox
          checked={enabled}
          name="auto_organize_enabled"
          label="Auto-organize completed torrents"
          help="Disabled by default. When enabled (and no custom action list), finished torrents are classified from name + filenames and moved under the organize root. Heuristics can be wrong."
          onChange={(e) =>
            onChange({ auto_organize_enabled: e.target.checked })
          }
        />
        <FormInput
          name="auto_organize_root"
          label="Auto-organize root"
          value={preferences.auto_organize_root ?? ""}
          placeholder="(empty = session download folder)"
          help="Base directory for type subfolders. Destination becomes root/<type>/<torrent-folder>."
          onChange={(e) =>
            onChange({ auto_organize_root: e.target.value || null })
          }
        />
      </Fieldset>

      <CollapsibleSection
        title="Type ? subfolder names"
        summary="Anime, TV, Movies, ?"
        defaultOpen={false}
      >
        <p className="text-sm text-tertiary mb-2">
          Customize the folder name used for each media type under the organize
          root.
        </p>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
          {folderKeys.map((key) => (
            <FormInput
              key={key}
              name={`folder_${key}`}
              label={key}
              value={
                preferences.auto_organize_folders?.[key] ??
                DEFAULT_ORGANIZE_FOLDERS[key]
              }
              onChange={(e) =>
                onChange({
                  auto_organize_folders: {
                    ...(preferences.auto_organize_folders ||
                      DEFAULT_ORGANIZE_FOLDERS),
                    [key]: e.target.value,
                  },
                })
              }
            />
          ))}
        </div>
      </CollapsibleSection>
    </div>
  );
};
