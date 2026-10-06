import { RemovePolicy, RuleAction, TorrentRules } from "../api-types";

export const DEFAULT_RULES = (): TorrentRules => ({
  stalled: { enabled: false, after_secs: 86400, action: "pause" },
  seeding: { enabled: false, action: "pause" },
  speed_window: { enabled: false, then: "cap", cap_kib_per_sec: 100 },
});

export const RULE_ACTION_LABELS: Record<RuleAction, string> = {
  pause: "Pause",
  flag: "Flag as needs attention",
  remove_keep: "Remove, keep files",
  remove_policy: "Remove per remove policy",
  remove_delete: "Remove and delete files",
  remove_finish: "Remove, finishing what's done",
};

export const STALLED_ACTIONS: RuleAction[] = [
  "flag",
  "pause",
  "remove_keep",
  "remove_finish",
  "remove_delete",
  "remove_policy",
];
export const SEEDING_ACTIONS: RuleAction[] = [
  "pause",
  "remove_keep",
  "remove_policy",
];

function actionPolicy(
  a: RuleAction,
  policy: RemovePolicy,
): RemovePolicy | null {
  switch (a) {
    case "pause":
    case "flag":
      return null;
    case "remove_keep":
      return { complete: "keep", incomplete: "keep" };
    case "remove_policy":
      return policy;
    case "remove_delete":
      return { complete: "delete", incomplete: "delete" };
    case "remove_finish":
      return { complete: "keep", incomplete: "finish" };
  }
}

/** Mirrors `RuleAction::may_delete` on the server. */
export function actionMayDelete(
  a: RuleAction,
  complete: boolean,
  policy: RemovePolicy,
): boolean {
  const p = actionPolicy(a, policy);
  if (!p) return false;
  return complete ? p.complete === "delete" : p.incomplete !== "keep";
}

/** Warnings for rule configurations that can delete files. */
export function ruleDeleteWarnings(
  rules: TorrentRules,
  policy: RemovePolicy,
): string[] {
  const w: string[] = [];
  if (rules.stalled.enabled && actionMayDelete(rules.stalled.action, false, policy)) {
    w.push(
      `Stalled torrents will be removed and files deleted (${RULE_ACTION_LABELS[rules.stalled.action].toLowerCase()}${
        rules.stalled.action === "remove_finish" ? ": unfinished files are deleted" : ""
      }).`,
    );
  }
  if (rules.seeding.enabled && actionMayDelete(rules.seeding.action, true, policy)) {
    w.push(
      "Torrents reaching a seeding limit will have their files deleted (remove policy deletes complete torrents).",
    );
  }
  return w;
}
