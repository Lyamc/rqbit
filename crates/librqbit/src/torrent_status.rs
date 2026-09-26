//! Server-computed, human-oriented torrent status ("status_detail" in stats).
//!
//! Every status here is derived from real engine state (torrent state machine, init
//! semaphore, queue holds, repair/backoff trackers, live peer/transfer counters); nothing
//! is guessed on the client.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Downloading torrents with no new data for this long are "stalled".
pub const STALL_SECS: u64 = 60;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    /// Waiting for a free initial-check slot (concurrent init limit).
    QueuedForChecking,
    /// Verifying existing data on disk.
    Checking,
    /// Metadata (info dict) not known yet.
    ResolvingMetadata,
    Initializing,
    Downloading,
    /// Downloading, but no data arrived for [`STALL_SECS`].
    Stalled,
    Seeding,
    /// Finished and paused (not seeding).
    Complete,
    Paused,
    Error,
    /// Automatic repair waiting for the global repair slot.
    QueuedForRepair,
    Repairing,
    /// Pieces held back after I/O errors; next attempt scheduled by backoff.
    WaitingToRetry,
    /// Automatic recovery gave up; manual Fix errors needed.
    NeedsAttention,
    Moving,
    Renaming,
    /// Held by queue limits (max active downloads / torrents).
    QueuedForDownloading,
    /// Held by queue limits (max active uploads / torrents).
    QueuedForSeeding,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StatusDetail {
    pub kind: StatusKind,
    /// Short human label, e.g. "Checking 42%", "Queued for downloading (#3)".
    pub label: String,
    /// 0..=1 for checking / repairing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_in_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<usize>,
}

impl StatusDetail {
    pub fn simple(kind: StatusKind, label: String) -> Self {
        Self { kind, label, progress: None, next_retry_in_secs: None, queue_position: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveOp {
    Moving,
    Renaming,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EngineState {
    Initializing {
        /// Holds an init slot and is reading data.
        checking: bool,
        /// Check was requested (task spawned) but may be waiting for a slot.
        check_requested: bool,
        user_paused: bool,
    },
    Live,
    Paused,
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveView {
    pub download_bps: u64,
    pub upload_bps: u64,
    pub peers_live: u32,
    /// Seconds since new data last arrived (while live).
    pub secs_since_data: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepairView {
    pub waiting_for_slot: bool,
    pub progress: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetryView {
    pub pieces_waiting: usize,
    pub next_retry_in_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusInputs<'a> {
    pub engine: EngineState,
    pub metadata_resolved: bool,
    pub finished: bool,
    /// Checked fraction while initializing.
    pub check_progress: f64,
    pub queue_held: bool,
    pub queue_position: Option<usize>,
    pub active_op: Option<ActiveOp>,
    pub repair: Option<RepairView>,
    pub needs_attention: bool,
    pub retry: Option<RetryView>,
    pub live: Option<LiveView>,
    pub error: Option<&'a str>,
}

/// Per-torrent runtime flags used for status derivation and queueing.
#[derive(Default)]
pub struct RuntimeFlags {
    queue_held: AtomicBool,
    active_op: Mutex<Option<ActiveOp>>,
    activity: Mutex<Option<(u64, Instant)>>,
}

pub struct OpGuard<'a>(&'a RuntimeFlags);

impl Drop for OpGuard<'_> {
    fn drop(&mut self) {
        *self.0.active_op.lock() = None;
    }
}

impl RuntimeFlags {
    /// Held by queue limits (paused internally, not by the user).
    pub fn queue_held(&self) -> bool {
        self.queue_held.load(Ordering::Acquire)
    }

    pub(crate) fn set_queue_held(&self, v: bool) {
        self.queue_held.store(v, Ordering::Release);
    }

    pub(crate) fn begin_op(&self, op: ActiveOp) -> OpGuard<'_> {
        *self.active_op.lock() = Some(op);
        OpGuard(self)
    }

    pub fn active_op(&self) -> Option<ActiveOp> {
        *self.active_op.lock()
    }

    /// Record the live torrent's fetched-bytes counter; returns seconds since it last
    /// changed.
    pub(crate) fn observe_fetched(&self, fetched: u64, now: Instant) -> u64 {
        let mut g = self.activity.lock();
        match *g {
            Some((last, since)) if last == fetched => now.duration_since(since).as_secs(),
            _ => {
                *g = Some((fetched, now));
                0
            }
        }
    }

    pub(crate) fn reset_activity(&self) {
        *self.activity.lock() = None;
    }
}

pub fn format_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs.div_ceil(60))
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    }
}

fn pct(p: f64) -> String {
    format!("{}%", (p.clamp(0.0, 1.0) * 100.0).floor() as u64)
}

pub fn derive_status(i: &StatusInputs<'_>) -> StatusDetail {
    let mk = |kind: StatusKind, label: String| StatusDetail {
        kind,
        label,
        progress: None,
        next_retry_in_secs: None,
        queue_position: i.queue_position,
    };

    if matches!(i.engine, EngineState::Error) {
        let short: String = i
            .error
            .unwrap_or("error")
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
        return mk(StatusKind::Error, format!("Error: {short}"));
    }
    if let Some(op) = i.active_op {
        return match op {
            ActiveOp::Moving => mk(StatusKind::Moving, "Moving files".into()),
            ActiveOp::Renaming => mk(StatusKind::Renaming, "Renaming".into()),
        };
    }
    if let Some(r) = i.repair {
        if r.waiting_for_slot {
            return mk(StatusKind::QueuedForRepair, "Queued for repair".into());
        }
        let mut s = mk(StatusKind::Repairing, format!("Repairing {}", pct(r.progress)));
        s.progress = Some(r.progress.clamp(0.0, 1.0));
        return s;
    }
    if i.needs_attention {
        return mk(
            StatusKind::NeedsAttention,
            "Needs attention (automatic recovery gave up)".into(),
        );
    }
    if !i.metadata_resolved {
        return mk(StatusKind::ResolvingMetadata, "Resolving metadata".into());
    }
    let queued_label = |what: &str| match i.queue_position {
        Some(p) => format!("Queued for {what} (#{p})"),
        None => format!("Queued for {what}"),
    };
    match i.engine {
        EngineState::Error => unreachable!(),
        EngineState::Initializing {
            checking,
            check_requested,
            user_paused,
        } => {
            if checking {
                let mut s = mk(
                    StatusKind::Checking,
                    format!("Checking {}", pct(i.check_progress)),
                );
                s.progress = Some(i.check_progress.clamp(0.0, 1.0));
                s
            } else if user_paused {
                mk(StatusKind::Paused, "Paused".into())
            } else if check_requested {
                mk(StatusKind::QueuedForChecking, "Queued for checking".into())
            } else {
                mk(StatusKind::Initializing, "Initializing".into())
            }
        }
        EngineState::Paused => {
            if i.queue_held {
                if i.finished {
                    mk(StatusKind::QueuedForSeeding, queued_label("seeding"))
                } else {
                    mk(StatusKind::QueuedForDownloading, queued_label("downloading"))
                }
            } else if i.finished {
                mk(StatusKind::Complete, "Complete".into())
            } else {
                mk(StatusKind::Paused, "Paused".into())
            }
        }
        EngineState::Live => {
            let lv = i.live.unwrap_or_default();
            if i.finished {
                return mk(StatusKind::Seeding, "Seeding".into());
            }
            let stalled = lv.secs_since_data >= STALL_SECS;
            if let Some(r) = i.retry.filter(|r| r.pieces_waiting > 0) {
                // Only the headline status when nothing else is moving.
                if stalled || lv.download_bps < 1024 {
                    let mut label = format!("Waiting to retry ({} piece(s)", r.pieces_waiting);
                    if let Some(n) = r.next_retry_in_secs {
                        label += &format!(", next attempt in {}", format_secs(n));
                    }
                    label.push(')');
                    let mut s = mk(StatusKind::WaitingToRetry, label);
                    s.next_retry_in_secs = r.next_retry_in_secs;
                    return s;
                }
                let mut s = mk(
                    StatusKind::Downloading,
                    format!("Downloading ({} piece(s) waiting to retry)", r.pieces_waiting),
                );
                s.next_retry_in_secs = r.next_retry_in_secs;
                return s;
            }
            if stalled {
                let label = if lv.peers_live == 0 {
                    "Stalled (no peers)".to_string()
                } else {
                    format!(
                        "Stalled (no data for {}, {} peer(s))",
                        format_secs(lv.secs_since_data),
                        lv.peers_live
                    )
                };
                return mk(StatusKind::Stalled, label);
            }
            mk(StatusKind::Downloading, "Downloading".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> StatusInputs<'static> {
        StatusInputs {
            engine: EngineState::Live,
            metadata_resolved: true,
            finished: false,
            check_progress: 0.0,
            queue_held: false,
            queue_position: Some(3),
            active_op: None,
            repair: None,
            needs_attention: false,
            retry: None,
            live: Some(LiveView {
                download_bps: 500_000,
                upload_bps: 0,
                peers_live: 4,
                secs_since_data: 1,
            }),
            error: None,
        }
    }

    fn kind(i: &StatusInputs<'_>) -> StatusKind {
        derive_status(i).kind
    }

    #[test]
    fn live_states() {
        let mut i = base();
        assert_eq!(kind(&i), StatusKind::Downloading);
        i.live.as_mut().unwrap().secs_since_data = STALL_SECS;
        let s = derive_status(&i);
        assert_eq!(s.kind, StatusKind::Stalled);
        assert_eq!(s.label, "Stalled (no data for 1m, 4 peer(s))");
        i.live.as_mut().unwrap().peers_live = 0;
        assert_eq!(derive_status(&i).label, "Stalled (no peers)");
        i.finished = true;
        assert_eq!(kind(&i), StatusKind::Seeding);
    }

    #[test]
    fn init_states() {
        let mut i = base();
        i.live = None;
        i.engine = EngineState::Initializing {
            checking: false,
            check_requested: true,
            user_paused: false,
        };
        assert_eq!(kind(&i), StatusKind::QueuedForChecking);
        i.engine = EngineState::Initializing {
            checking: true,
            check_requested: true,
            user_paused: false,
        };
        i.check_progress = 0.426;
        let s = derive_status(&i);
        assert_eq!((s.kind, s.label.as_str()), (StatusKind::Checking, "Checking 42%"));
        assert_eq!(s.progress, Some(0.426));
        i.engine = EngineState::Initializing {
            checking: false,
            check_requested: false,
            user_paused: true,
        };
        assert_eq!(kind(&i), StatusKind::Paused);
        i.engine = EngineState::Initializing {
            checking: false,
            check_requested: false,
            user_paused: false,
        };
        assert_eq!(kind(&i), StatusKind::Initializing);
        i.metadata_resolved = false;
        assert_eq!(kind(&i), StatusKind::ResolvingMetadata);
    }

    #[test]
    fn paused_queue_and_complete() {
        let mut i = base();
        i.live = None;
        i.engine = EngineState::Paused;
        assert_eq!(kind(&i), StatusKind::Paused);
        i.finished = true;
        assert_eq!(kind(&i), StatusKind::Complete);
        i.queue_held = true;
        let s = derive_status(&i);
        assert_eq!(s.kind, StatusKind::QueuedForSeeding);
        assert_eq!(s.label, "Queued for seeding (#3)");
        i.finished = false;
        assert_eq!(derive_status(&i).label, "Queued for downloading (#3)");
    }

    #[test]
    fn recovery_and_repair_priorities() {
        let mut i = base();
        i.retry = Some(RetryView {
            pieces_waiting: 5,
            next_retry_in_secs: Some(125),
        });
        // Still downloading other pieces: stays Downloading, with a note.
        let s = derive_status(&i);
        assert_eq!(s.kind, StatusKind::Downloading);
        assert!(s.label.contains("5 piece(s) waiting to retry"));
        // Nothing else moving: Waiting to retry is the headline.
        i.live.as_mut().unwrap().download_bps = 0;
        let s = derive_status(&i);
        assert_eq!(s.kind, StatusKind::WaitingToRetry);
        assert_eq!(s.label, "Waiting to retry (5 piece(s), next attempt in 3m)");
        assert_eq!(s.next_retry_in_secs, Some(125));
        i.needs_attention = true;
        assert_eq!(kind(&i), StatusKind::NeedsAttention);
        i.repair = Some(RepairView {
            waiting_for_slot: true,
            progress: 0.0,
        });
        assert_eq!(kind(&i), StatusKind::QueuedForRepair);
        i.repair = Some(RepairView {
            waiting_for_slot: false,
            progress: 0.175,
        });
        assert_eq!(derive_status(&i).label, "Repairing 17%");
        i.active_op = Some(ActiveOp::Moving);
        assert_eq!(kind(&i), StatusKind::Moving);
        i.active_op = Some(ActiveOp::Renaming);
        assert_eq!(kind(&i), StatusKind::Renaming);
        i.engine = EngineState::Error;
        i.error = Some("error writing\nmore");
        let s = derive_status(&i);
        assert_eq!((s.kind, s.label.as_str()), (StatusKind::Error, "Error: error writing"));
    }

    #[test]
    fn serializes_snake_case() {
        let s = derive_status(&base());
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["kind"], "downloading");
        assert_eq!(j["queue_position"], 3);
        assert_eq!(
            serde_json::to_value(StatusKind::QueuedForDownloading).unwrap(),
            "queued_for_downloading"
        );
    }

    #[test]
    fn secs_formatting() {
        assert_eq!(format_secs(5), "5s");
        assert_eq!(format_secs(61), "2m");
        assert_eq!(format_secs(3600), "1h");
        assert_eq!(format_secs(5400), "1h 30m");
    }
}
