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
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AddedTorrentDetails {
    pub name: Option<String>,
    pub info_hash: String,
}

/// `GET /add_jobs/{id}`: what the server is doing with an in-flight add.
/// `stage` is one of starting, fetching_torrent, resolving_metadata,
/// adopting, waiting_for_server, adding, added, already_managed, list_only,
/// failed, cancelled.
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
