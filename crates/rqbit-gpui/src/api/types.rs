//! Subset of the rqbit HTTP API response types used by the GUI.
//!
//! These mirror `crates/librqbit/webui/src/api-types.ts` (the web UI's view of
//! the same API). Everything is lenient (`#[serde(default)]`, optional fields)
//! so the client keeps working against older/newer servers, and so we don't
//! have to depend on `librqbit` (which would drag the whole server into the
//! client build).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ListTorrentsResponse {
    pub torrents: Vec<TorrentListItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentListItem {
    pub id: usize,
    pub info_hash: String,
    pub name: Option<String>,
    pub output_folder: String,
    pub total_pieces: u32,
    pub stats: Option<TorrentStats>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TorrentState {
    #[default]
    Initializing,
    Paused,
    Live,
    Error,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentStats {
    pub state: TorrentState,
    pub error: Option<String>,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub finished: bool,
    pub live: Option<LiveTorrentStats>,
    /// Server-computed detailed status (this fork's addition).
    pub status_detail: Option<StatusDetail>,
    pub queue_position: Option<u32>,
    pub repair_count: Option<u64>,
    /// Present when files are damaged (unreadable) or a repair ran.
    pub damage: Option<DamageStats>,
    /// Bytes downloaded per file, same order as `TorrentDetails::files`.
    pub file_progress: Vec<u64>,
}

/// Mirrors `DamageStats` in api-types.ts (fields the UI uses).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DamageStats {
    pub damaged_files: Vec<DamagedFileStats>,
    pub repair: Option<RepairStatus>,
    pub recovery: Option<RecoveryStats>,
    pub needs_attention: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DamagedFileStats {
    pub file_id: usize,
    pub path: String,
    pub errors: u64,
    pub eio: bool,
    pub last_error: String,
    pub pieces_failed: u64,
    pub auto_repair_attempts: Option<u32>,
    pub next_auto_repair_in_secs: Option<u64>,
    pub needs_attention: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RecoveryStats {
    pub max_attempts: u32,
    pub pieces_waiting: u32,
    pub pieces_needing_attention: u32,
    pub max_piece_attempts: u32,
    pub next_retry_in_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepairStatus {
    /// "running" | "done" | "failed"
    pub state: String,
    pub auto: bool,
    pub scanned_bytes: u64,
    pub total_bytes: u64,
    pub files_total: u32,
    pub files_done: u32,
    pub current_file: Option<String>,
    pub summary: Option<RepairSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepairSummary {
    pub files_scanned: u32,
    pub files_repaired: u32,
    pub files_failed: u32,
    pub bytes_unreadable: u64,
    pub bytes_zeroed: u64,
    pub pieces_to_redownload: u64,
    pub pieces_invalidated: u64,
    pub files: Vec<FileRepairOutcome>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileRepairOutcome {
    pub file_id: usize,
    pub path: String,
    pub method: String,
}

impl DamageStats {
    pub fn has_damaged_files(&self) -> bool {
        !self.damaged_files.is_empty()
    }
    /// Pieces held back / given up after I/O errors, or files whose auto repair gave up.
    pub fn has_recovery_issues(&self) -> bool {
        self.recovery.is_some() || self.needs_attention == Some(true)
    }
    pub fn is_repair_running(&self) -> bool {
        self.repair.as_ref().is_some_and(|r| r.state == "running")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LiveTorrentStats {
    pub snapshot: StatsSnapshot,
    pub download_speed: Speed,
    pub upload_speed: Speed,
    pub time_remaining: Option<TimeRemaining>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatsSnapshot {
    pub have_bytes: u64,
    pub fetched_bytes: u64,
    pub uploaded_bytes: u64,
    pub remaining_bytes: u64,
    pub total_bytes: u64,
    pub peer_stats: AggregatePeerStats,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AggregatePeerStats {
    pub queued: u32,
    pub connecting: u32,
    pub live: u32,
    pub seen: u32,
    pub dead: u32,
    pub not_needed: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Speed {
    /// MiB/s.
    pub mbps: f64,
    pub human_readable: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TimeRemaining {
    pub human_readable: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StatusDetail {
    /// e.g. "downloading", "seeding", "checking", "needs_attention", ...
    pub kind: String,
    /// Human readable label computed by the server.
    pub label: String,
    pub progress: Option<f64>,
    pub next_retry_in_secs: Option<u64>,
    pub queue_position: Option<u32>,
}

/// Error body returned by the rqbit API on failure.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ApiErrorBody {
    pub human_readable: Option<String>,
    pub error_kind: Option<String>,
}

/// `POST /torrents` response (the parts the GUI uses).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AddTorrentResponse {
    pub id: Option<usize>,
    pub details: AddedTorrentDetails,
    pub output_folder: String,
    /// Magnet queued; its metadata is resolving in the background (a success).
    #[serde(default)]
    pub resolving: bool,
    /// Deferred magnet add: the info hash was already in rqbit (or resolving).
    #[serde(default)]
    pub already_managed: bool,
    /// `resolving_metadata`, `added`, `already_managed` or `list_only` (newer servers).
    #[serde(default)]
    pub state: Option<String>,
    /// Added paused for this Add window; started when the window closes.
    #[serde(default)]
    pub held: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AddedTorrentDetails {
    pub name: Option<String>,
    pub info_hash: String,
}

/// `GET /add_jobs/{id}`: what the server is doing with an in-flight add.
/// `stage` is one of starting, fetching_torrent, resolving_metadata,
/// adopting, waiting_for_server, adding, added, already_managed,
/// resolving_in_background, list_only, failed, cancelled.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AddJobStatus {
    pub job_id: Option<String>,
    pub stage: String,
    pub torrent_id: Option<usize>,
    pub step: Option<String>,
    pub busy: Option<bool>,
    pub error: Option<String>,
    pub reason: Option<String>,
    pub stage_secs: f64,
    pub elapsed_secs: f64,
}

/// `POST /add_jobs/{id}/cancel`: `result` is cancelled, already_added or
/// finished.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AddJobCancelOutcome {
    pub result: String,
    pub torrent_id: Option<usize>,
    pub stage: Option<String>,
}

/// `GET/POST /torrents/limits` (bytes per second; None = unlimited).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct LimitsConfig {
    #[serde(default)]
    pub upload_bps: Option<u64>,
    #[serde(default)]
    pub download_bps: Option<u64>,
}

/// `GET /admin`. `persisted` (admin.json) is kept as raw JSON.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AdminStatus {
    pub version: String,
    pub preferences_path: String,
    pub admin_path: String,
    pub effective_http_listen_addr: Option<String>,
    pub env_http_listen_addr: Option<String>,
    pub env_basic_auth_set: bool,
    pub persisted: serde_json::Map<String, serde_json::Value>,
    pub restart_supported: bool,
    pub notes: Vec<String>,
}

impl TorrentListItem {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.info_hash.clone())
    }
}

impl TorrentStats {
    /// 0.0..=1.0
    pub fn progress_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            return if self.finished { 1.0 } else { 0.0 };
        }
        (self.progress_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
    }

    /// Upload ratio for this session: bytes uploaded since the torrent was
    /// (re)started divided by the bytes we have. rqbit only tracks upload
    /// counters per session, so this is not an all-time ratio.
    pub fn session_ratio(&self) -> Option<f64> {
        let live = self.live.as_ref()?;
        let have = if live.snapshot.have_bytes > 0 {
            live.snapshot.have_bytes
        } else {
            self.progress_bytes
        };
        if have == 0 {
            return None;
        }
        Some(live.snapshot.uploaded_bytes as f64 / have as f64)
    }

    pub fn status_kind(&self) -> &str {
        match &self.status_detail {
            Some(d) if !d.kind.is_empty() => &d.kind,
            _ => match self.state {
                TorrentState::Initializing => "initializing",
                TorrentState::Paused => "paused",
                TorrentState::Error => "error",
                TorrentState::Live if self.finished => "seeding",
                TorrentState::Live => "downloading",
                TorrentState::Unknown => "unknown",
            },
        }
    }

    pub fn status_label(&self) -> String {
        if let Some(d) = &self.status_detail
            && !d.label.is_empty()
        {
            return d.label.clone();
        }
        match self.state {
            TorrentState::Initializing => "Checking files".into(),
            TorrentState::Paused => "Paused".into(),
            TorrentState::Error => "Error".into(),
            TorrentState::Live if self.finished => "Seeding".into(),
            TorrentState::Live => "Downloading".into(),
            TorrentState::Unknown => "Unknown".into(),
        }
    }

    /// Mirrors the web UI: a paused or errored torrent can be started, a live
    /// or initializing one can be paused.
    pub fn can_start(&self) -> bool {
        matches!(self.state, TorrentState::Paused | TorrentState::Error)
    }

    pub fn can_pause(&self) -> bool {
        matches!(self.state, TorrentState::Live | TorrentState::Initializing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_with_detailed_status() {
        let body = r#"{"torrents":[{"id":3,"info_hash":"aa","name":"Foo","output_folder":"/d",
            "total_pieces":10,"stats":{"state":"live","error":null,"file_progress":[1],
            "progress_bytes":50,"finished":false,"total_bytes":100,
            "live":{"snapshot":{"have_bytes":50,"downloaded_and_checked_bytes":50,"fetched_bytes":60,
              "uploaded_bytes":25,"initially_needed_bytes":100,"remaining_bytes":50,"total_bytes":100,
              "total_piece_download_ms":1,"peer_stats":{"queued":0,"connecting":0,"live":2,"seen":5,"dead":0,"not_needed":0}},
              "average_piece_download_time":{"secs":0,"nanos":0},
              "download_speed":{"mbps":1.5,"human_readable":"1.50 MiB/s"},
              "upload_speed":{"mbps":0.0,"human_readable":"0.00 MiB/s"},
              "all_time_download_speed":{"mbps":1.0,"human_readable":"1.00 MiB/s"},
              "time_remaining":null},
            "status_detail":{"kind":"downloading","label":"Downloading"},"some_future_field":1}}]}"#;
        let r: ListTorrentsResponse = serde_json::from_str(body).unwrap();
        let t = &r.torrents[0];
        let s = t.stats.as_ref().unwrap();
        assert_eq!(t.display_name(), "Foo");
        assert_eq!(s.state, TorrentState::Live);
        assert_eq!(s.status_label(), "Downloading");
        assert!((s.progress_fraction() - 0.5).abs() < 1e-9);
        assert!((s.session_ratio().unwrap() - 0.5).abs() < 1e-9);
        assert!(s.can_pause() && !s.can_start());
    }

    #[test]
    fn parses_add_job_types() {
        let st: AddJobStatus = serde_json::from_str(
            r#"{"job_id":"add-1","stage":"adding","torrent_id":4,"step":"saving","busy":true,"stage_secs":1.5,"elapsed_secs":3.0}"#,
        )
        .unwrap();
        assert_eq!(st.stage, "adding");
        assert_eq!(st.torrent_id, Some(4));
        assert_eq!(st.busy, Some(true));
        let c: AddJobCancelOutcome =
            serde_json::from_str(r#"{"result":"already_added","torrent_id":7}"#).unwrap();
        assert_eq!(
            (c.result.as_str(), c.torrent_id),
            ("already_added", Some(7))
        );
        let c: AddJobCancelOutcome =
            serde_json::from_str(r#"{"result":"finished","stage":"failed","error":"x"}"#).unwrap();
        assert_eq!(c.stage.as_deref(), Some("failed"));
        let r: AddTorrentResponse = serde_json::from_str(
            r#"{"id":3,"details":{"name":"n","info_hash":"ab","files":[],"output_folder":"/o"},"output_folder":"/o","seen_peers":null}"#,
        )
        .unwrap();
        assert_eq!(r.id, Some(3));
        assert_eq!(r.details.name.as_deref(), Some("n"));
        let l: LimitsConfig = serde_json::from_str(r#"{"upload_bps":null}"#).unwrap();
        assert_eq!(l, LimitsConfig::default());
    }

    #[test]
    fn tolerates_old_server_without_stats_or_status_detail() {
        let body = r#"{"torrents":[{"id":1,"info_hash":"bb","name":null,"output_folder":"/x"},
            {"id":2,"info_hash":"cc","name":"P","output_folder":"/x","stats":{"state":"paused",
             "error":null,"progress_bytes":0,"total_bytes":0,"finished":false,"live":null}},
            {"id":4,"info_hash":"dd","name":"Q","output_folder":"/x","stats":{"state":"weird"}}]}"#;
        let r: ListTorrentsResponse = serde_json::from_str(body).unwrap();
        assert_eq!(r.torrents[0].display_name(), "bb");
        let p = r.torrents[1].stats.as_ref().unwrap();
        assert_eq!(p.status_label(), "Paused");
        assert!(p.can_start());
        assert!(p.session_ratio().is_none());
        assert_eq!(
            r.torrents[2].stats.as_ref().unwrap().state,
            TorrentState::Unknown
        );
    }
}

// ---- details / peers / events / session / filesystem (api-types.ts) ----

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentFileAttributes {
    pub symlink: bool,
    pub hidden: bool,
    pub padding: bool,
    pub executable: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentFile {
    pub name: String,
    pub components: Vec<String>,
    pub length: u64,
    pub included: bool,
    pub attributes: TorrentFileAttributes,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentDetails {
    pub name: Option<String>,
    pub info_hash: String,
    pub files: Vec<TorrentFile>,
    pub total_pieces: Option<u32>,
    pub output_folder: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PeerCounters {
    pub incoming_connections: u64,
    pub fetched_bytes: u64,
    pub uploaded_bytes: u64,
    pub total_time_connecting_ms: u64,
    pub connection_attempts: u64,
    pub connections: u64,
    pub errors: u64,
    pub fetched_chunks: u64,
    pub downloaded_and_checked_pieces: u64,
    pub total_piece_download_ms: u64,
    pub times_stolen_from_me: u64,
    pub times_i_stole: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PeerStats {
    pub counters: PeerCounters,
    pub state: String,
    pub conn_kind: Option<String>,
    pub client_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PeerStatsSnapshot {
    pub peers: std::collections::BTreeMap<String, PeerStats>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EventRecord {
    pub seq: u64,
    pub time: String,
    pub kind: String,
    /// "info" | "warning" | "error"
    pub severity: String,
    pub torrent_id: Option<usize>,
    pub info_hash: Option<String>,
    pub torrent_name: Option<String>,
    pub file_id: Option<usize>,
    pub path: Option<String>,
    pub message: String,
    pub count: Option<u64>,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EventPage {
    pub events: Vec<EventRecord>,
    pub next_before_seq: Option<u64>,
    pub latest_seq: u64,
}

#[derive(Debug, Clone, Default)]
pub struct EventQuery {
    pub kind: Option<String>,
    pub torrent_id: Option<usize>,
    pub info_hash: Option<String>,
    pub severity: Option<String>,
    pub before_seq: Option<u64>,
    pub since_seq: Option<u64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepairCounters {
    pub since: String,
    pub auto_repairs: u64,
    pub manual_repairs: u64,
    pub repair_failures: u64,
    pub files_repaired: u64,
    pub bytes_unreadable: u64,
    pub bytes_zeroed: u64,
    pub bytes_redownload: u64,
    pub pieces_requeued: u64,
    pub give_ups: u64,
    pub piece_retries: u64,
    pub io_errors: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UnseenCounts {
    pub repairs: u64,
    pub errors: u64,
    pub warnings: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EventSummary {
    pub counters: RepairCounters,
    pub latest_seq: u64,
    pub unseen: UnseenCounts,
    pub log_bytes: u64,
    pub log_cap_bytes: u64,
    pub log_segments: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SessionCounters {
    pub fetched_bytes: u64,
    pub uploaded_bytes: u64,
    pub blocked_incoming: u64,
    pub blocked_outgoing: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SessionStats {
    pub counters: SessionCounters,
    pub peers: AggregatePeerStats,
    pub download_speed: Speed,
    pub upload_speed: Speed,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FsRoot {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FsRootsResponse {
    pub roots: Vec<FsRoot>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_torrent: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FsListResponse {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<FsEntry>,
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PublicIpFamily {
    pub ip: Option<String>,
    pub source: Option<String>,
    pub error: Option<String>,
}

/// `GET /public_ip`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PublicIp {
    pub enabled: bool,
    pub ipv4: PublicIpFamily,
    pub ipv6: PublicIpFamily,
    pub checked_at: Option<String>,
    pub age_secs: Option<u64>,
    pub checking: bool,
}

/// What Remove does with files (`remove_policy` preference / remove endpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovePolicy {
    /// "keep" | "delete"
    pub complete: &'static str,
    /// "keep" | "delete" | "finish"
    pub incomplete: &'static str,
}

impl Default for RemovePolicy {
    fn default() -> Self {
        Self::KEEP
    }
}

impl RemovePolicy {
    pub const KEEP: RemovePolicy = RemovePolicy {
        complete: "keep",
        incomplete: "keep",
    };

    /// From stored strings (unknown values fall back to keep).
    pub fn from_strs(complete: Option<&str>, incomplete: Option<&str>) -> Self {
        RemovePolicy {
            complete: match complete {
                Some("delete") => "delete",
                _ => "keep",
            },
            incomplete: match incomplete {
                Some("delete") => "delete",
                Some("finish") => "finish",
                _ => "keep",
            },
        }
    }

    pub fn from_json(v: Option<&serde_json::Value>) -> Self {
        Self::from_strs(
            v.and_then(|v| v.get("complete")).and_then(|v| v.as_str()),
            v.and_then(|v| v.get("incomplete")).and_then(|v| v.as_str()),
        )
    }

    /// Whether removing these groups deletes anything.
    pub fn deletes_files(&self, any_complete: bool, any_incomplete: bool) -> bool {
        (any_complete && self.complete == "delete") || (any_incomplete && self.incomplete != "keep")
    }

    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({"complete": self.complete, "incomplete": self.incomplete})
    }
}

/// `GET /torrents/remove_preview?ids=…` item.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RemovePreviewItem {
    pub id: usize,
    pub name: String,
    pub complete: bool,
    pub files_complete: usize,
    pub files_partial: usize,
    pub bytes_complete: u64,
    pub bytes_partial: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RemovePreview {
    pub policy: Option<serde_json::Value>,
    pub confirm_remove: bool,
    pub completion_actions: Vec<String>,
    pub items: Vec<RemovePreviewItem>,
    pub complete: usize,
    pub incomplete: usize,
    pub incomplete_nothing_done: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RemoveOutcome {
    pub id: usize,
    pub name: String,
    pub result: String,
    pub deleted_files: Vec<String>,
    pub actions_run: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentCounters {
    pub seeding_secs: u64,
    pub uploaded_total: u64,
    pub idle_secs: u64,
}

/// `GET /torrents/{id}/rules`. Rules are kept as JSON (edited generically).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TorrentRulesView {
    pub global: serde_json::Value,
    #[serde(rename = "override")]
    pub override_: Option<serde_json::Value>,
    pub effective: serde_json::Value,
    pub counters: TorrentCounters,
    pub ratio: f64,
    pub status: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileOrderView {
    pub id: usize,
    pub name: String,
    pub sequential: bool,
    pub first_last_first: bool,
    #[serde(rename = "override")]
    pub override_: serde_json::Value,
}

/// `GET /torrents/{id}/download_order`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DownloadOrderView {
    pub global: serde_json::Value,
    pub torrent: serde_json::Value,
    pub effective: serde_json::Value,
    pub files: Vec<FileOrderView>,
    pub summary: String,
}

// ---- Orphaned-download cleanup (`/cleanup/*`) ----

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct CleanupRoot {
    pub path: String,
    /// "download" | "completion" | "organize" | "custom".
    pub kind: String,
    pub default_on: bool,
    pub exists: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CleanupScanSummary {
    pub scan_id: String,
    pub time: u64,
    pub items: usize,
    pub total_bytes: u64,
    pub scheduled: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CleanupRootsResponse {
    pub roots: Vec<CleanupRoot>,
    pub min_age_minutes: u64,
    pub scan_hours: Option<u64>,
    pub allowed_parents: Vec<String>,
    pub latest_scan: Option<CleanupScanSummary>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CleanupItem {
    pub id: u32,
    pub path: String,
    pub root: String,
    /// "file" | "dir".
    pub kind: String,
    pub size: u64,
    pub mtime: Option<u64>,
    pub files: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CleanupSkipped {
    pub recent: u64,
    pub symlinks: u64,
    pub hidden: u64,
    pub errors: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CleanupScan {
    pub scan_id: String,
    pub time: u64,
    pub roots: Vec<String>,
    pub min_age_minutes: u64,
    pub items: Vec<CleanupItem>,
    pub skipped: CleanupSkipped,
    pub total_bytes: u64,
    pub torrents_checked: usize,
    pub scheduled: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CleanupItemResult {
    pub id: Option<u32>,
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CleanupApplyOutcome {
    pub action: String,
    pub ok: usize,
    pub failed: usize,
    pub bytes: u64,
    pub results: Vec<CleanupItemResult>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct QuarantineItem {
    pub original: String,
    pub stored: String,
    pub kind: String,
    pub size: u64,
    pub files: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct QuarantineBatch {
    pub id: String,
    pub root: String,
    pub dir: String,
    pub created: u64,
    pub items: Vec<QuarantineItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct QuarantineList {
    pub batches: Vec<QuarantineBatch>,
}
