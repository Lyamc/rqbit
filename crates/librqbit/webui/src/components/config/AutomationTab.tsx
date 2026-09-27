import React from "react";
import { SessionPreferences } from "../../api-types";
import { DEFAULT_RULES } from "../../helper/rules";
import { policyFromPrefs } from "../../helper/removePrefs";
import { Fieldset } from "../forms/Fieldset";
import {
  RuleWarnings,
  SeedingEditor,
  StalledEditor,
  WindowEditor,
} from "./RulesEditor";

export { DEFAULT_RULES };

export const AutomationTab: React.FC<{
  preferences: SessionPreferences;
  onChange: (patch: Partial<SessionPreferences>) => void;
}> = ({ preferences, onChange }) => {
  const rules = preferences.rules ?? DEFAULT_RULES();
  const policy = policyFromPrefs(preferences);
  return (
    <div className="text-secondary py-2 space-y-4">
      <p className="text-sm text-tertiary">
        Automatic rules, all off by default. These are the global defaults;
        each torrent can override them in its details (Rules tab). Counters
        (seeding time, uploaded, idle time) survive restarts, and every time a
        rule fires it is written to the Events log. Rules only delete files
        when a delete action is chosen here.
      </p>
      <RuleWarnings rules={rules} policy={policy} />
      <Fieldset label="Stalled / no progress">
        <StalledEditor
          id="prefs"
          value={rules.stalled}
          onChange={(stalled) => onChange({ rules: { ...rules, stalled } })}
        />
      </Fieldset>
      <Fieldset label="Seeding limits">
        <SeedingEditor
          id="prefs"
          value={rules.seeding}
          onChange={(seeding) => onChange({ rules: { ...rules, seeding } })}
        />
      </Fieldset>
      <Fieldset label="Full-speed window">
        <WindowEditor
          id="prefs"
          value={rules.speed_window}
          onChange={(speed_window) =>
            onChange({ rules: { ...rules, speed_window } })
          }
        />
      </Fieldset>
      <p className="text-sm text-tertiary">
        "Remove per remove policy" uses Interface → Removing torrents (currently:
        complete → {policy.complete}, incomplete → {policy.incomplete}).
        Seeding rotation is under BitTorrent → Queueing.
      </p>
    </div>
  );
};
