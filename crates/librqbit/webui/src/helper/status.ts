import { StatusDetail, StatusKind, TorrentStats } from "../api-types";

/** Badge colors per server-computed status kind. */
export const STATUS_KIND_CLASS: Record<StatusKind, string> = {
  queued_for_checking: "bg-surface-sunken text-secondary",
  checking: "bg-warning-bg/20 text-warning",
  resolving_metadata: "bg-warning-bg/20 text-warning",
  initializing: "bg-warning-bg/20 text-warning",
  downloading: "bg-primary-bg/20 text-primary",
  stalled: "bg-warning-bg/20 text-warning",
  seeding: "bg-success-bg/20 text-success",
  complete: "bg-success-bg/10 text-success",
  paused: "bg-surface-sunken text-secondary",
  error: "bg-error-bg/20 text-error",
  queued_for_repair: "bg-warning-bg/20 text-warning",
  repairing: "bg-warning-bg/20 text-warning",
  waiting_to_retry: "bg-warning-bg/20 text-warning",
  needs_attention: "bg-error-bg/20 text-error",
  moving: "bg-primary-bg/20 text-primary",
  renaming: "bg-primary-bg/20 text-primary",
  queued_for_downloading: "bg-surface-sunken text-tertiary",
  queued_for_seeding: "bg-surface-sunken text-tertiary",
};

/** Fallback for older servers without status_detail. */
export const statusDetailOf = (s?: TorrentStats | null): StatusDetail | null => {
  if (!s) return null;
  if (s.status_detail) return s.status_detail;
  if (s.error) return { kind: "error", label: "Error" };
  switch (s.state) {
    case "initializing":
      return { kind: "initializing", label: "Initializing" };
    case "paused":
      return s.finished
        ? { kind: "complete", label: "Complete" }
        : { kind: "paused", label: "Paused" };
    case "live":
      return s.finished
        ? { kind: "seeding", label: "Seeding" }
        : { kind: "downloading", label: "Downloading" };
    default:
      return null;
  }
};

export const statusClass = (d: StatusDetail | null): string =>
  d ? STATUS_KIND_CLASS[d.kind] ?? "text-secondary" : "text-secondary";

/** Sort key: groups similar states together. */
const STATUS_ORDER: StatusKind[] = [
  "error",
  "needs_attention",
  "repairing",
  "queued_for_repair",
  "waiting_to_retry",
  "moving",
  "renaming",
  "checking",
  "queued_for_checking",
  "resolving_metadata",
  "initializing",
  "downloading",
  "stalled",
  "queued_for_downloading",
  "seeding",
  "queued_for_seeding",
  "complete",
  "paused",
];

export const statusSortValue = (s?: TorrentStats | null): number => {
  const d = statusDetailOf(s);
  return d ? STATUS_ORDER.indexOf(d.kind) : 999;
};
