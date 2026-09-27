// Interface for the Torrent API response
export interface TorrentId {
  id: number;
  info_hash: string;
}

export interface TorrentFile {
  name: string;
  components: string[];
  length: number;
  included: boolean;
  attributes: TorrentFileAttributes;
}

export interface TorrentFileAttributes {
  symlink: boolean;
  hidden: boolean;
  padding: boolean;
  executable: boolean;
}

// Interface for the Torrent Details API response (with files, from individual endpoint)
export interface TorrentDetails {
  name: string | null;
  info_hash: string;
  files: Array<TorrentFile>;
  total_pieces?: number;
  output_folder: string;
}

// Interface for torrent list item (from bulk /torrents?with_stats=true endpoint)
// This matches TorrentDetailsResponse from the backend, but files are not included in the list
export interface TorrentListItem {
  id: number;
  info_hash: string;
  name: string | null;
  output_folder: string;
  total_pieces: number;
  stats?: TorrentStats;
}

export interface AddTorrentResponse {
  id: number | null;
  details: TorrentDetails;
  output_folder: string;
  seen_peers?: Array<string>;
  /** Magnet queued (the default for magnets): its metadata is resolving in the
   *  background and the torrent is listed as "Resolving metadata". A success. For a
   *  `list_only` preview (HTTP 202) it means "no metadata yet" and `id` is null. */
  resolving?: boolean;
  /** Deferred magnet whose info hash was already in rqbit (or resolving). */
  already_managed?: boolean;
  info_hash?: string;
  state?: "resolving_metadata" | "added" | "already_managed" | "list_only";
}

export interface ListTorrentsResponse {
  torrents: Array<TorrentListItem>;
}

export interface Speed {
  mbps: number;
  human_readable: string;
}

export interface AggregatePeerStats {
  queued: number;
  connecting: number;
  live: number;
  seen: number;
  dead: number;
  not_needed: number;
}

export type ConnectionKind = "tcp" | "utp" | "socks";

export interface PeerCounters {
  incoming_connections: number;
  fetched_bytes: number;
  uploaded_bytes: number;
  total_time_connecting_ms: number;
  connection_attempts: number;
  connections: number;
  errors: number;
  fetched_chunks: number;
  downloaded_and_checked_pieces: number;
  total_piece_download_ms: number;
  times_stolen_from_me: number;
  times_i_stole: number;
}

export interface PeerStats {
  counters: PeerCounters;
  state: string;
  conn_kind: ConnectionKind | null;
  client_name: string | null;
}

export interface PeerStatsSnapshot {
  peers: Record<string, PeerStats>;
}

export interface ConnectionStatSingle {
  attempts: number;
  successes: number;
  errors: number;
}
export interface ConnectionStatsPerFamily {
  v4: ConnectionStatSingle;
  v6: ConnectionStatSingle;
}
export interface ConnectionStats {
  tcp: ConnectionStatsPerFamily;
  utp: ConnectionStatsPerFamily;
  socks: ConnectionStatsPerFamily;
}

export interface SessionCounters {
  fetched_bytes: number;
  uploaded_bytes: number;
  blocked_incoming: number;
  blocked_outgoing: number;
}

export interface SessionStats {
  counters: SessionCounters;
  peers: AggregatePeerStats;
  connections: ConnectionStats;
  download_speed: Speed;
  upload_speed: Speed;
  uptime_seconds: number;
}

export interface AdminConfigPublic {
  http_api_listen_addr?: string | null;
  basic_auth_enabled: boolean;
  basic_auth_user?: string | null;
  basic_auth_password_set: boolean;
  listen_port?: number | null;
  announce_port?: number | null;
  disable_dht?: boolean | null;
  disable_dht_persistence?: boolean | null;
  disable_lsd?: boolean | null;
  disable_trackers?: boolean | null;
  enable_utp_listen?: boolean | null;
  disable_tcp_listen?: boolean | null;
  disable_tcp_connect?: boolean | null;
  disable_upnp_port_forward?: boolean | null;
  socks_proxy_url?: string | null;
  ipv4_only?: boolean | null;
  bind_device?: string | null;
  peer_limit?: number | null;
  concurrent_init_limit?: number | null;
  peer_connect_timeout_secs?: number | null;
  peer_read_write_timeout_secs?: number | null;
  blocklist_url?: string | null;
  allowlist_url?: string | null;
  fastresume?: boolean | null;
}

export interface AdminStatus {
  version: string;
  preferences_path: string;
  admin_path: string;
  effective_http_listen_addr?: string | null;
  env_http_listen_addr?: string | null;
  env_basic_auth_set: boolean;
  persisted: AdminConfigPublic;
  restart_supported: boolean;
  notes: string[];
}

export interface AdminConfigUpdate {
  http_api_listen_addr?: string | null;
  basic_auth_enabled?: boolean | null;
  basic_auth_user?: string | null;
  basic_auth_password?: string | null;
  listen_port?: number | null;
  clear_listen_port?: boolean | null;
  announce_port?: number | null;
  clear_announce_port?: boolean | null;
  disable_dht?: boolean | null;
  clear_disable_dht?: boolean | null;
  disable_dht_persistence?: boolean | null;
  clear_disable_dht_persistence?: boolean | null;
  disable_lsd?: boolean | null;
  clear_disable_lsd?: boolean | null;
  disable_trackers?: boolean | null;
  clear_disable_trackers?: boolean | null;
  enable_utp_listen?: boolean | null;
  clear_enable_utp_listen?: boolean | null;
  disable_tcp_listen?: boolean | null;
  clear_disable_tcp_listen?: boolean | null;
  disable_tcp_connect?: boolean | null;
  clear_disable_tcp_connect?: boolean | null;
  disable_upnp_port_forward?: boolean | null;
  clear_disable_upnp_port_forward?: boolean | null;
  socks_proxy_url?: string | null;
  ipv4_only?: boolean | null;
  clear_ipv4_only?: boolean | null;
  bind_device?: string | null;
  peer_limit?: number | null;
  clear_peer_limit?: boolean | null;
  concurrent_init_limit?: number | null;
  clear_concurrent_init_limit?: boolean | null;
  peer_connect_timeout_secs?: number | null;
  clear_peer_connect_timeout_secs?: boolean | null;
  peer_read_write_timeout_secs?: number | null;
  clear_peer_read_write_timeout_secs?: boolean | null;
  blocklist_url?: string | null;
  allowlist_url?: string | null;
  fastresume?: boolean | null;
  clear_fastresume?: boolean | null;
}

export interface LimitsConfig {
  upload_bps?: number | null;
  download_bps?: number | null;
}

export type CompletionActionType =
  "shell" | "move" | "organize" | "drop_incomplete_ext";

export interface CompletionAction {
  type: CompletionActionType;
  command?: string;
  path?: string;
  copy?: boolean;
}

export interface AutoOrganizeFolders {
  anime: string;
  tv: string;
  movie: string;
  game: string;
  porn: string;
  music: string;
  book: string;
  software: string;
  other: string;
}

export interface SessionPreferences {
  soft_recover_on_io_error: boolean;
  /** Auto-run "Repair damaged files" when soft recovery marks a file damaged. */
  auto_repair_damaged_files?: boolean;
  /** Automatic recovery backoff: first retry delay (s), doubling per failure. */
  recovery_backoff_base_secs?: number;
  /** Automatic recovery backoff cap (s). */
  recovery_backoff_cap_secs?: number;
  /** Stop automatic recovery after this many consecutive failures. */
  recovery_max_attempts?: number;
  /** Total size cap of the event log in MiB (default 10). */
  event_log_max_mb?: number;
  /** Queueing (active limits). Off by default. */
  queueing_enabled?: boolean;
  queue_max_active_downloads?: number | null;
  queue_max_active_uploads?: number | null;
  queue_max_active_torrents?: number | null;
  queue_ignore_slow_torrents?: boolean;
  on_complete_hook?: string | null;
  move_completed_path?: string | null;
  move_completed_copy?: boolean;
  auto_organize_enabled?: boolean;
  auto_organize_root?: string | null;
  auto_organize_folders?: AutoOrganizeFolders;
  incomplete_extension?: string | null;
  completion_actions?: CompletionAction[];
  /** Live default max peers per newly added torrent. */
  peer_limit?: number | null;
  /** UI: ask before removing torrents (default true). Deleting files always asks. */
  confirm_remove?: boolean;
  /** Legacy single remove action (read-only; migrated to remove_policy by the server). */
  default_remove_action?: RemoveAction;
  /** What Remove does with files, for complete and incomplete torrents. */
  remove_policy?: RemovePolicy;
  /** Automatic rules (stalled, seeding limits, full-speed window). All off by default. */
  rules?: TorrentRules;
  /** Rotate seeding slots every N seconds (needs queueing + max active uploads). */
  queue_seed_rotation_secs?: number | null;
  /** Default download order (piece picker). */
  download_order?: DownloadOrderDefaults;
  cleanup_scan_hours?: number | null;
  cleanup_min_age_minutes?: number | null;
  cleanup_extra_roots?: string[];
}

export type RemoveAction = "keep_files" | "delete_files";

export type CompleteRemoveAction = "keep" | "delete";
export type IncompleteRemoveAction = "keep" | "delete" | "finish";
export interface RemovePolicy {
  complete: CompleteRemoveAction;
  incomplete: IncompleteRemoveAction;
}

export interface RemoveOutcome {
  id: number;
  name: string;
  was_complete: boolean;
  policy: RemovePolicy;
  result: string;
  deleted_files: string[];
  deleted_bytes: number;
  kept_files: number;
  actions_run: string[];
  final_folder?: string;
}

// ---- Orphan cleanup
export interface CleanupRoot {
  path: string;
  kind: "download" | "move" | "organize" | "custom";
  default_on: boolean;
  exists: boolean;
}
export interface CleanupScanSummary {
  scan_id: string;
  time: number;
  items: number;
  total_bytes: number;
  scheduled: boolean;
}
export interface CleanupRootsResponse {
  roots: CleanupRoot[];
  min_age_minutes: number;
  scan_hours: number | null;
  allowed_parents: string[];
  latest_scan: CleanupScanSummary | null;
}
export interface CleanupItem {
  id: number;
  path: string;
  root: string;
  kind: "file" | "dir";
  size: number;
  mtime: number | null;
  files: number;
  reason: string;
}
export interface CleanupScan {
  scan_id: string;
  time: number;
  roots: string[];
  min_age_minutes: number;
  items: CleanupItem[];
  skipped: {
    recent: number;
    symlinks: number;
    hidden: number;
    errors: string[];
    truncated: boolean;
  };
  total_bytes: number;
  torrents_checked: number;
  protected_folders: number;
  scheduled: boolean;
  duration_ms: number;
}
export interface CleanupItemResult {
  id: number | null;
  path: string;
  ok: boolean;
  error?: string;
  moved_to?: string;
  size: number;
}
export interface CleanupApplyOutcome {
  action: "quarantine" | "delete";
  ok: number;
  failed: number;
  bytes: number;
  batches?: string[];
  results: CleanupItemResult[];
}
export interface QuarantineBatch {
  id: string;
  root: string;
  dir: string;
  created: number;
  items: {
    original: string;
    stored: string;
    kind: string;
    size: number;
    files: number;
  }[];
}

export interface RemovePreviewItem {
  id: number;
  name: string;
  complete: boolean;
  files_complete: number;
  files_partial: number;
  bytes_complete: number;
  bytes_partial: number;
}

export interface RemovePreview {
  policy: RemovePolicy;
  confirm_remove: boolean;
  completion_actions: string[];
  items: RemovePreviewItem[];
  complete: number;
  incomplete: number;
  incomplete_nothing_done: number;
}

export type RuleAction =
  | "pause"
  | "flag"
  | "remove_keep"
  | "remove_policy"
  | "remove_delete"
  | "remove_finish";

export interface StalledRule {
  enabled: boolean;
  after_secs: number;
  action: RuleAction;
}

export interface SeedingLimits {
  enabled: boolean;
  max_seed_secs?: number | null;
  max_uploaded_bytes?: number | null;
  max_ratio?: number | null;
  action: RuleAction;
}

export interface SpeedWindow {
  enabled: boolean;
  full_speed_secs?: number | null;
  full_speed_bytes?: number | null;
  then: "cap" | "stop";
  cap_kib_per_sec: number;
}

export interface TorrentRules {
  stalled: StalledRule;
  seeding: SeedingLimits;
  speed_window: SpeedWindow;
}

export interface RulesOverride {
  stalled?: StalledRule | null;
  seeding?: SeedingLimits | null;
  speed_window?: SpeedWindow | null;
}

export interface TorrentCounters {
  seeding_secs: number;
  uploaded_total: number;
  idle_secs: number;
  completed_unix?: number;
}

export interface TorrentRulesView {
  global: TorrentRules;
  override: RulesOverride | null;
  effective: TorrentRules;
  counters: TorrentCounters;
  ratio: number;
  status: string[];
  warnings: string[];
}

export type FileOrder = "name" | "torrent" | "smallest_first" | "largest_first";

export interface DownloadOrderDefaults {
  sequential_files: boolean;
  file_order: FileOrder;
  sequential: boolean;
  first_last_first: boolean;
}

export interface FileOrderSettings {
  sequential?: boolean;
  first_last_first?: boolean;
}

export interface TorrentDownloadOrder {
  sequential_files?: boolean;
  file_order?: FileOrder;
  sequential?: boolean;
  first_last_first?: boolean;
  files?: Record<string, FileOrderSettings>;
}

export interface DownloadOrderView {
  global: DownloadOrderDefaults;
  torrent: TorrentDownloadOrder;
  effective: DownloadOrderDefaults;
  files: {
    id: number;
    name: string;
    sequential: boolean;
    first_last_first: boolean;
    override: FileOrderSettings;
  }[];
  summary: string;
}

/** Absent = unchanged, null = inherit. */
export interface DownloadOrderPatch {
  sequential_files?: boolean | null;
  file_order?: FileOrder | null;
  sequential?: boolean | null;
  first_last_first?: boolean | null;
  files?: {
    ids: number[];
    sequential?: boolean | null;
    first_last_first?: boolean | null;
  }[];
  reset?: boolean;
}

// Interface for the Torrent Stats API response
export interface LiveTorrentStats {
  snapshot: {
    have_bytes: number;
    downloaded_and_checked_bytes: number;
    downloaded_and_checked_pieces: number;
    fetched_bytes: number;
    uploaded_bytes: number;
    initially_needed_bytes: number;
    remaining_bytes: number;
    total_bytes: number;
    total_piece_download_ms: number;
    peer_stats: AggregatePeerStats;
  };
  average_piece_download_time: {
    secs: number;
    nanos: number;
  };
  download_speed: Speed;
  upload_speed: Speed;
  all_time_download_speed: {
    mbps: number;
    human_readable: string;
  };
  time_remaining: {
    human_readable: string;
    duration?: {
      secs: number;
    };
  } | null;
}

export const STATE_INITIALIZING = "initializing";
export const STATE_PAUSED = "paused";
export const STATE_LIVE = "live";
export const STATE_ERROR = "error";

export interface DamagedFileStats {
  file_id: number;
  path: string;
  errors: number;
  /** At least one failure was an OS-level I/O error (EIO). */
  eio: boolean;
  last_error: string;
  first_seen: string;
  last_seen: string;
  pieces_failed: number;
  /** Consecutive automatic repair attempts. */
  auto_repair_attempts?: number;
  next_auto_repair_in_secs?: number;
  /** Automatic repair gave up for this file. */
  needs_attention?: boolean;
}

export interface PieceBackoffStats {
  piece: number;
  attempts: number;
  next_retry_in_secs?: number;
  needs_attention: boolean;
  last_error: string;
}

export interface RecoveryStats {
  max_attempts: number;
  pieces_waiting: number;
  pieces_needing_attention: number;
  max_piece_attempts: number;
  next_retry_in_secs?: number;
  pieces: PieceBackoffStats[];
}

export type RepairMethod = "none" | "punch_hole" | "copy_replace" | "failed";

export interface FileRepairOutcome {
  file_id: number;
  path: string;
  bytes_total: number;
  bytes_unreadable: number;
  bytes_zeroed: number;
  ranges_zeroed: [number, number][];
  pieces: number[];
  method: RepairMethod;
  note?: string;
  error?: string;
}

export interface RepairSummary {
  files_scanned: number;
  files_repaired: number;
  files_failed: number;
  bytes_unreadable: number;
  bytes_zeroed: number;
  pieces_to_redownload: number;
  pieces_invalidated: number;
  files: FileRepairOutcome[];
}

export interface RepairStatus {
  state: "running" | "done" | "failed";
  auto: boolean;
  started_at: string;
  finished_at?: string;
  scanned_bytes: number;
  total_bytes: number;
  files_total: number;
  files_done: number;
  current_file?: string;
  summary?: RepairSummary;
  error?: string;
}

export interface DamageStats {
  damaged_files: DamagedFileStats[];
  repair?: RepairStatus;
  /** Automatic re-download of pieces after I/O errors (with backoff). */
  recovery?: RecoveryStats;
  /** Automatic recovery gave up somewhere; manual Fix errors needed. */
  needs_attention?: boolean;
}

export interface RepairStartResponse {
  started: boolean;
  files: number;
  total_bytes: number;
}

export type StatusKind =
  | "queued_for_checking"
  | "checking"
  | "resolving_metadata"
  | "initializing"
  | "downloading"
  | "stalled"
  | "seeding"
  | "complete"
  | "paused"
  | "error"
  | "queued_for_repair"
  | "repairing"
  | "waiting_to_retry"
  | "needs_attention"
  | "moving"
  | "renaming"
  | "queued_for_downloading"
  | "queued_for_seeding";

/** Server-computed detailed status. */
export interface StatusDetail {
  kind: StatusKind;
  label: string;
  progress?: number;
  next_retry_in_secs?: number;
  queue_position?: number;
}

export type QueueMoveAction = "up" | "down" | "top" | "bottom";

export interface TorrentStats {
  state: "initializing" | "paused" | "live" | "error";
  error: string | null;
  file_progress: number[];
  progress_bytes: number;
  finished: boolean;
  initializing_paused?: boolean;
  total_bytes: number;
  live: LiveTorrentStats | null;
  /** Present when files are damaged (unreadable) or a repair ran. */
  damage?: DamageStats;
  /** Detailed status computed by the server. */
  status_detail?: StatusDetail;
  /** 1-based queue position. */
  queue_position?: number;
  /** Repair runs on this torrent since the counters were reset. */
  repair_count?: number;
}

/** Client-side request options (not sent to the server). */
export interface RequestOptions {
  /** Abort the in-flight HTTP request (client side only; the server may still finish the add). */
  signal?: AbortSignal;
}

export interface ErrorDetails {
  id?: number;
  method?: string;
  path?: string;
  status?: number;
  statusText?: string;
  text: string | React.ReactNode;
}

export type Duration = number;

export interface PeerConnectionOptions {
  connect_timeout?: Duration | null;
  read_write_timeout?: Duration | null;
  keep_alive_interval?: Duration | null;
}

export interface AddTorrentOptions {
  paused?: boolean;
  only_files_regex?: string | null;
  only_files?: number[] | null;
  overwrite?: boolean;
  list_only?: boolean;
  output_folder?: string | null;
  sub_folder?: string | null;
  peer_opts?: PeerConnectionOptions | null;
  force_tracker_interval?: Duration | null;
  initial_peers?: string[] | null; // Assuming SocketAddr is equivalent to a string in TypeScript
  preferred_id?: number | null;
  /** Transfer from another client: adopt its files in output_folder before
   *  the initial check ("auto": a unique `<name><suffix>` partial that passes
   *  a piece-hash sample is renamed; "qbit" is an alias). */
  adopt_foreign_incomplete?: "auto" | "qbit" | null;
  /** Poll `GET /add_jobs/{id}` / cancel via `POST /add_jobs/{id}/cancel`. */
  add_job_id?: string;
  /** Magnets: length of one metadata resolve attempt; afterwards the server backs
   *  off and keeps trying (never fails the add). */
  magnet_timeout_secs?: number | null;
  /** No effect on current servers (magnets are always queued at once with a torrent
   *  id and resolved in the background); older servers needed it. */
  defer_metadata?: boolean;
  /** Magnets: wait for metadata before answering (old behaviour); if it doesn't
   *  arrive in time the magnet is queued anyway (`resolving: true`), never an error. */
  wait_for_metadata?: boolean;
}

export type Value = string | number | boolean;

export interface Span {
  name: string;
  [key: string]: Value;
}

/*
Example log line

const EXAMPLE_LOG_JSON: JSONLogLine = {
  timestamp: "2023-12-08T21:48:13.649165Z",
  level: "DEBUG",
  fields: { message: "successfully port forwarded 192.168.0.112:4225" },
  target: "librqbit_upnp",
  span: { port: 4225, name: "manage_port" },
  spans: [
    { port: 4225, name: "upnp_forward" },
    {
      location: "http://192.168.0.1:49152/IGDdevicedesc_brlan0.xml",
      name: "upnp_endpoint",
    },
    { device: "ARRIS TG3492LG", name: "device" },
    { device: "WANDevice:1", name: "device" },
    { device: "WANConnectionDevice:1", name: "device" },
    { url: "/upnp/control/WANIPConnection0", name: "service" },
    { port: 4225, name: "manage_port" },
  ],
};
*/
export interface JSONLogLine {
  level: string;
  timestamp: string;
  fields: {
    message: string;
    [key: string]: Value;
  };
  target: string;
  span: Span;
  spans: Span[];
}

export interface FsRoot {
  label: string;
  path: string;
}

export interface FsRootsResponse {
  roots: FsRoot[];
}

export interface FsEntry {
  name: string;
  path: string;
  is_dir: boolean;
  is_torrent: boolean;
  size?: number;
}

export interface FsListResponse {
  path: string;
  parent?: string | null;
  entries: FsEntry[];
  truncated?: boolean;
}

export interface ExtractItem {
  name: string;
  kind: string;
  data_base64?: string;
  magnet?: string;
  error?: string;
}

export interface ExtractResponse {
  items: ExtractItem[];
}

/** What the server is doing for an in-flight add (`GET /add_jobs/{id}`). */
export type AddJobStage =
  | "starting"
  | "fetching_torrent"
  | "resolving_metadata"
  | "adopting"
  | "waiting_for_server"
  | "adding"
  | "added"
  | "already_managed"
  | "resolving_in_background"
  | "list_only"
  | "failed"
  | "cancelled";

export interface AddJobAdoptSummary {
  reused: number;
  renamed: number;
  ambiguous: string[];
  rejected: string[];
  skipped_target_exists: string[];
}

export interface AddJobStatus {
  job_id?: string | null;
  stage: AddJobStage;
  torrent_id?: number;
  /** "adding" only: opening_files | saving | starting. */
  step?: string;
  /** "adding" only: all disk I/O slots were busy (other torrents checking). */
  busy?: boolean;
  error?: string;
  reason?: string;
  stage_secs: number;
  elapsed_secs: number;
  adopt?: AddJobAdoptSummary;
}

export type AddJobCancelOutcome =
  | { result: "cancelled" }
  | { result: "already_added"; torrent_id: number }
  | ({ result: "finished" } & Partial<AddJobStatus>);

export interface RqbitAPI {
  getPlaylistUrl: (index: number) => string | null;
  getStreamLogsUrl: () => string | null;
  listTorrents: (opts?: {
    withStats?: boolean;
  }) => Promise<ListTorrentsResponse>;
  getTorrentDetails: (index: number) => Promise<TorrentDetails>;
  getTorrentStats: (index: number) => Promise<TorrentStats>;
  getTorrentHaves: (index: number) => Promise<Uint8Array>;
  getPeerStats: (index: number) => Promise<PeerStatsSnapshot>;
  getTorrentStreamUrl: (
    index: number,
    file_id: number,
    filename?: string | null,
  ) => string | null;
  uploadTorrent: (
    data: string | File,
    opts?: AddTorrentOptions,
    init?: RequestOptions,
  ) => Promise<AddTorrentResponse>;
  uploadTorrentFromServerPath: (
    path: string,
    opts?: AddTorrentOptions,
    init?: RequestOptions,
  ) => Promise<AddTorrentResponse>;
  /** Status of an add started with `add_job_id` (404 until it arrives). */
  getAddJob?: (jobId: string) => Promise<AddJobStatus>;
  /** Cancel an add started with `add_job_id`. */
  cancelAddJob?: (jobId: string) => Promise<AddJobCancelOutcome>;
  fsRoots: () => Promise<FsRootsResponse>;
  fsList: (
    path: string,
    opts?: { recursive?: boolean; torrentsOnly?: boolean },
  ) => Promise<FsListResponse>;
  extractUpload: (data: Blob | ArrayBuffer | File) => Promise<ExtractResponse>;

  pause: (index: number) => Promise<void>;
  updateOnlyFiles: (index: number, files: number[]) => Promise<void>;
  renameFile: (index: number, fileId: number, newPath: string) => Promise<void>;
  relocateTorrent: (
    index: number,
    destination: string,
    copy?: boolean,
  ) => Promise<void>;
  start: (index: number) => Promise<void>;
  restart: (index: number) => Promise<void>;
  fixErrors: (index: number) => Promise<void>;
  /** Move torrents in the queue (multi-select). Returns the new order (ids). */
  queueMove?: (
    ids: number[],
    action: QueueMoveAction,
  ) => Promise<{ order: number[] }>;
  /** Event log (repairs, recovery failures, I/O errors). */
  getEvents?: (q: EventQuery) => Promise<EventPage>;
  getEventsSummary?: (sinceSeq?: number) => Promise<EventSummary>;
  /** Server's public IPv4/IPv6 as seen from its own network (e.g. VPN exit). */
  getPublicIp?: (refresh?: boolean) => Promise<PublicIpInfo>;
  /**
   * host:port the browser talks to (default port made explicit). Browsers
   * don't expose the resolved IP or the local port, so that's all we show.
   */
  getConnectionTarget?: () => string | null;
  resetEventCounters?: () => Promise<RepairCounters>;
  /** Scan the torrent's files for unreadable ranges and repair them (background job). */
  repairFiles?: (
    index: number,
    opts?: { files?: number[]; scope?: "damaged" | "all" },
  ) => Promise<RepairStartResponse>;
  forget: (index: number) => Promise<void>;
  delete: (index: number) => Promise<void>;
  /** Remove with a policy (null = saved default). */
  remove?: (
    index: number,
    policy: RemovePolicy | null,
    opts?: { wait?: boolean },
  ) => Promise<RemoveOutcome>;
  removePreview?: (ids: number[]) => Promise<RemovePreview>;
  cleanupRoots?: () => Promise<CleanupRootsResponse>;
  cleanupScan?: (
    roots: string[] | null,
    minAgeMinutes?: number,
  ) => Promise<CleanupScan>;
  cleanupScanResult?: (scanId?: string) => Promise<CleanupScan>;
  cleanupApply?: (
    scanId: string,
    itemIds: number[],
    action: "quarantine" | "delete",
    confirm: boolean,
  ) => Promise<CleanupApplyOutcome>;
  cleanupQuarantine?: () => Promise<{ batches: QuarantineBatch[] }>;
  cleanupRestore?: (
    batch: string,
    items?: number[],
  ) => Promise<{ results: CleanupItemResult[] }>;
  cleanupPurge?: (
    batch: string,
  ) => Promise<{ items: number; bytes: number }>;
  recheck?: (index: number) => Promise<void>;
  getTorrentRules?: (index: number) => Promise<TorrentRulesView>;
  setTorrentRules?: (
    index: number,
    override: RulesOverride | null,
  ) => Promise<TorrentRulesView>;
  getDownloadOrder?: (index: number) => Promise<DownloadOrderView>;
  setDownloadOrder?: (
    index: number,
    patch: DownloadOrderPatch,
  ) => Promise<DownloadOrderView>;
  stats: () => Promise<SessionStats>;
  getLimits: () => Promise<LimitsConfig>;
  setLimits: (limits: LimitsConfig) => Promise<void>;
  getPreferences: () => Promise<SessionPreferences>;
  setPreferences: (prefs: SessionPreferences) => Promise<void>;
  getAdminStatus: () => Promise<AdminStatus>;
  updateAdminConfig: (patch: AdminConfigUpdate) => Promise<AdminConfigPublic>;
  reloadPreferences: () => Promise<SessionPreferences>;
  restartProcess: () => Promise<void>;
}

export type EventSeverity = "info" | "warning" | "error";

export interface EventRecord {
  seq: number;
  time: string;
  kind: string;
  severity: EventSeverity;
  torrent_id?: number;
  info_hash?: string;
  torrent_name?: string;
  file_id?: number;
  path?: string;
  message: string;
  /** Occurrences this record stands for (aggregated errors); 1 when omitted. */
  count?: number;
  details?: any;
}

export interface EventQuery {
  kind?: string;
  torrent_id?: number;
  info_hash?: string;
  severity?: EventSeverity;
  since?: string;
  since_seq?: number;
  before_seq?: number;
  limit?: number;
}

export interface EventPage {
  events: EventRecord[];
  next_before_seq: number | null;
  latest_seq: number;
}

export interface RepairCounters {
  since: string;
  auto_repairs: number;
  manual_repairs: number;
  repair_failures: number;
  files_repaired: number;
  bytes_unreadable: number;
  bytes_zeroed: number;
  bytes_redownload: number;
  pieces_requeued: number;
  give_ups: number;
  piece_retries: number;
  io_errors: number;
  per_torrent: Record<string, number>;
}

export interface EventSummary {
  counters: RepairCounters;
  latest_seq: number;
  unseen: { repairs: number; errors: number; warnings: number; total: number };
  log_bytes: number;
  log_cap_bytes: number;
  log_segments: number;
}

export interface PublicIpFamily {
  ip: string | null;
  source: string | null;
  error: string | null;
}

export interface PublicIpInfo {
  enabled: boolean;
  ipv4: PublicIpFamily;
  ipv6: PublicIpFamily;
  checked_at: string | null;
  age_secs: number | null;
  check_interval_secs: number;
  checking: boolean;
}
