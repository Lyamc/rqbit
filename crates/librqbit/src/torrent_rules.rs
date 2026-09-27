//! Automatic per-torrent rules (all off by default):
//!
//! - **Stalled**: an incomplete torrent that made no verified download progress for N
//!   seconds of *running* time (paused time doesn't count) gets paused, flagged, or removed.
//! - **Seeding limits**: stop seeding when seeding time, uploaded amount or ratio reaches a
//!   limit (any enabled limit).
//! - **Full-speed window**: seed uncapped for the first N seconds / bytes after completion,
//!   then cap this torrent's upload rate or stop.
//!
//! Global defaults live in preferences (`rules`); a torrent may override any of the three
//! (persisted with its counters in `torrent-rules.json`). Counters (seeding time, uploaded
//! bytes, idle time) survive restarts.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::remove_policy::{CompleteRemoveAction, IncompleteRemoveAction, RemovePolicy};

/// Ratio with Eq (compared bitwise), so preferences can keep deriving Eq.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct Ratio(pub f64);
impl Eq for Ratio {}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    #[default]
    Pause,
    /// Mark "needs attention" (stalled rule only).
    Flag,
    /// Remove the torrent, keep all files.
    RemoveKeep,
    /// Remove using the saved remove policy (may delete files if the policy does).
    RemovePolicy,
    /// Remove the torrent and delete all its files.
    RemoveDelete,
    /// Remove, finishing what's done (stalled rule only): keep verified files, delete
    /// partial ones, run the completion actions.
    RemoveFinish,
}

impl RuleAction {
    pub fn label(&self) -> &'static str {
        match self {
            RuleAction::Pause => "pause",
            RuleAction::Flag => "flag as needs attention",
            RuleAction::RemoveKeep => "remove (keep files)",
            RuleAction::RemovePolicy => "remove (remove policy)",
            RuleAction::RemoveDelete => "remove and delete files",
            RuleAction::RemoveFinish => "remove, finishing what's done",
        }
    }

    /// The explicit remove policy for remove actions (`RemovePolicy` resolves to `default`).
    pub fn remove_policy(&self, default: RemovePolicy) -> Option<RemovePolicy> {
        match self {
            RuleAction::Pause | RuleAction::Flag => None,
            RuleAction::RemoveKeep => Some(RemovePolicy::KEEP),
            RuleAction::RemovePolicy => Some(default),
            RuleAction::RemoveDelete => Some(RemovePolicy::DELETE),
            RuleAction::RemoveFinish => Some(RemovePolicy {
                complete: CompleteRemoveAction::Keep,
                incomplete: IncompleteRemoveAction::Finish,
            }),
        }
    }

    /// Whether firing this action on a torrent (complete or not) can delete files.
    pub fn may_delete(&self, complete: bool, default: RemovePolicy) -> bool {
        match self.remove_policy(default) {
            None => false,
            Some(p) => {
                if complete {
                    p.complete == CompleteRemoveAction::Delete
                } else {
                    p.incomplete != IncompleteRemoveAction::Keep
                }
            }
        }
    }
}

fn default_stalled_secs() -> u64 {
    86400
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct StalledRule {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_stalled_secs")]
    pub after_secs: u64,
    #[serde(default)]
    pub action: RuleAction,
}

impl Default for StalledRule {
    fn default() -> Self {
        Self { enabled: false, after_secs: default_stalled_secs(), action: RuleAction::Pause }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeedingLimits {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_seed_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uploaded_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ratio: Option<Ratio>,
    #[serde(default)]
    pub action: RuleAction,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowThen {
    /// Cap this torrent's upload rate.
    #[default]
    Cap,
    /// Stop seeding (pause).
    Stop,
}

fn default_cap_kib() -> u32 {
    100
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeedWindow {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_speed_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_speed_bytes: Option<u64>,
    #[serde(default)]
    pub then: WindowThen,
    #[serde(default = "default_cap_kib")]
    pub cap_kib_per_sec: u32,
}

impl Default for SpeedWindow {
    fn default() -> Self {
        Self {
            enabled: false,
            full_speed_secs: None,
            full_speed_bytes: None,
            then: WindowThen::Cap,
            cap_kib_per_sec: default_cap_kib(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TorrentRules {
    #[serde(default)]
    pub stalled: StalledRule,
    #[serde(default)]
    pub seeding: SeedingLimits,
    #[serde(default)]
    pub speed_window: SpeedWindow,
}

impl TorrentRules {
    pub fn sanitize(mut self) -> Self {
        self.stalled.after_secs = self.stalled.after_secs.max(10);
        // Seeding: flag / finish make no sense for a complete torrent.
        self.seeding.action = match self.seeding.action {
            RuleAction::Flag => RuleAction::Pause,
            RuleAction::RemoveFinish => RuleAction::RemoveKeep,
            a => a,
        };
        let pos = |v: Option<u64>| v.filter(|v| *v > 0);
        self.seeding.max_seed_secs = pos(self.seeding.max_seed_secs);
        self.seeding.max_uploaded_bytes = pos(self.seeding.max_uploaded_bytes);
        self.seeding.max_ratio = self.seeding.max_ratio.filter(|r| r.0.is_finite() && r.0 > 0.0);
        self.speed_window.full_speed_secs = pos(self.speed_window.full_speed_secs);
        self.speed_window.full_speed_bytes = pos(self.speed_window.full_speed_bytes);
        self.speed_window.cap_kib_per_sec = self.speed_window.cap_kib_per_sec.max(1);
        self
    }

    pub fn with_override(&self, o: &RulesOverride) -> TorrentRules {
        TorrentRules {
            stalled: o.stalled.unwrap_or(self.stalled),
            seeding: o.seeding.unwrap_or(self.seeding),
            speed_window: o.speed_window.unwrap_or(self.speed_window),
        }
        .sanitize()
    }

    /// Human warnings for rule configurations that can delete files.
    pub fn delete_warnings(&self, policy: RemovePolicy) -> Vec<String> {
        let mut w = Vec::new();
        if self.stalled.enabled && self.stalled.action.may_delete(false, policy) {
            w.push(format!(
                "Stalled torrents will be removed with file deletion ({}).",
                self.stalled.action.label()
            ));
        }
        if self.seeding.enabled && self.seeding.action.may_delete(true, policy) {
            w.push(format!(
                "Torrents reaching a seeding limit will have their files deleted ({}).",
                self.seeding.action.label()
            ));
        }
        w
    }
}

/// Per-torrent override: `None` = use the global rule.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RulesOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled: Option<StalledRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeding: Option<SeedingLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_window: Option<SpeedWindow>,
}

impl RulesOverride {
    pub fn is_empty(&self) -> bool {
        self.stalled.is_none() && self.seeding.is_none() && self.speed_window.is_none()
    }
}

/// Persisted per-torrent counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TorrentCounters {
    /// Seconds spent seeding (running and complete).
    #[serde(default)]
    pub seeding_secs: u64,
    /// Bytes uploaded, all time (across restarts).
    #[serde(default)]
    pub uploaded_total: u64,
    /// Running seconds without verified download progress (reset on progress).
    #[serde(default)]
    pub idle_secs: u64,
    #[serde(default)]
    pub last_progress_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_unix: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_unix: Option<i64>,
    #[serde(default)]
    pub uploaded_at_complete: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled_fired_unix: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeding_fired_unix: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_done_unix: Option<i64>,
    #[serde(default)]
    pub window_stop_fired: bool,
    /// The stalled rule fired and is latched until progress resumes or the torrent is
    /// paused/queued (so a "flag" action doesn't fire again every period).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stalled_latched: bool,
}

/// What the rules engine sees of a torrent in one tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct Observation {
    /// Running (not paused, not queue-held).
    pub live: bool,
    pub finished: bool,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub uploaded_delta: u64,
    pub now_unix: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Stalled,
    Seeding,
    SpeedWindow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firing {
    pub rule: RuleKind,
    /// None = informational only (window ended with a cap).
    pub action: Option<RuleAction>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalResult {
    pub firings: Vec<Firing>,
    /// Upload cap this torrent should have now (bytes/s), from the full-speed window.
    pub upload_cap_bps: Option<u32>,
    /// Verified progress resumed (clears a "stalled" attention flag).
    pub progressed: bool,
}

pub fn ratio(c: &TorrentCounters, total_bytes: u64) -> f64 {
    c.uploaded_total as f64 / total_bytes.max(1) as f64
}

/// Advance counters by `dt` seconds and decide which rules fire.
pub fn evaluate(r: &TorrentRules, c: &mut TorrentCounters, o: &Observation, dt: u64) -> EvalResult {
    let mut res = EvalResult::default();
    c.uploaded_total += o.uploaded_delta;

    // Progress / stalled.
    if o.progress_bytes > c.last_progress_bytes {
        if c.last_progress_unix.is_some() || c.last_progress_bytes > 0 {
            res.progressed = true;
        }
        c.last_progress_bytes = o.progress_bytes;
        c.last_progress_unix = Some(o.now_unix);
        c.idle_secs = 0;
        c.stalled_latched = false;
    } else if o.progress_bytes < c.last_progress_bytes {
        // Recheck / deselected files: restart from here.
        c.last_progress_bytes = o.progress_bytes;
        c.idle_secs = 0;
    } else if o.live && !o.finished {
        c.idle_secs += dt;
    }
    if o.finished || !o.live {
        c.stalled_latched = false;
    }
    if o.finished {
        c.idle_secs = 0;
    }
    if r.stalled.enabled
        && !o.finished
        && o.live
        && !c.stalled_latched
        && c.idle_secs >= r.stalled.after_secs
    {
        res.firings.push(Firing {
            rule: RuleKind::Stalled,
            action: Some(r.stalled.action),
            reason: format!(
                "No download progress for {} of running time",
                fmt_secs(c.idle_secs)
            ),
        });
        c.idle_secs = 0;
        c.stalled_fired_unix = Some(o.now_unix);
        c.stalled_latched = true;
    }

    // Completion bookkeeping.
    if o.finished {
        if c.completed_unix.is_none() {
            c.completed_unix = Some(o.now_unix);
            c.uploaded_at_complete = c.uploaded_total;
            c.seeding_secs = 0;
            c.seeding_fired_unix = None;
            c.window_done_unix = None;
            c.window_stop_fired = false;
        }
        if o.live {
            c.seeding_secs += dt;
        }
    } else if c.completed_unix.is_some() {
        // More files selected again: it's downloading, not seeding.
        c.completed_unix = None;
    }

    if o.finished {
        let s = &r.seeding;
        if s.enabled && c.seeding_fired_unix.is_none() {
            let mut hit = Vec::new();
            if let Some(m) = s.max_seed_secs.filter(|m| c.seeding_secs >= *m) {
                hit.push(format!("seeded for {}", fmt_secs(m)));
            }
            if let Some(m) = s.max_uploaded_bytes.filter(|m| c.uploaded_total >= *m) {
                hit.push(format!("uploaded {}", fmt_bytes(m)));
            }
            if let Some(m) = s.max_ratio.filter(|m| ratio(c, o.total_bytes) >= m.0) {
                hit.push(format!("ratio {:.2}", m.0));
            }
            if !hit.is_empty() {
                c.seeding_fired_unix = Some(o.now_unix);
                res.firings.push(Firing {
                    rule: RuleKind::Seeding,
                    action: Some(s.action),
                    reason: format!("Seeding limit reached: {}", hit.join(", ")),
                });
            }
        }

        let w = &r.speed_window;
        if w.enabled && (w.full_speed_secs.is_some() || w.full_speed_bytes.is_some()) {
            let since = c.uploaded_total.saturating_sub(c.uploaded_at_complete);
            if c.window_done_unix.is_none() {
                let t = w.full_speed_secs.is_some_and(|m| c.seeding_secs >= m);
                let b = w.full_speed_bytes.is_some_and(|m| since >= m);
                if t || b {
                    c.window_done_unix = Some(o.now_unix);
                    let reason = format!(
                        "Full-speed seeding window over ({} seeding, {} uploaded since completion)",
                        fmt_secs(c.seeding_secs),
                        fmt_bytes(since)
                    );
                    res.firings.push(Firing {
                        rule: RuleKind::SpeedWindow,
                        action: match w.then {
                            WindowThen::Cap => None,
                            WindowThen::Stop => Some(RuleAction::Pause),
                        },
                        reason: match w.then {
                            WindowThen::Cap => format!("{reason}: capping upload at {} KiB/s", w.cap_kib_per_sec),
                            WindowThen::Stop => format!("{reason}: stopping seeding"),
                        },
                    });
                    if w.then == WindowThen::Stop {
                        c.window_stop_fired = true;
                    }
                }
            }
            if c.window_done_unix.is_some() && w.then == WindowThen::Cap {
                res.upload_cap_bps = Some(w.cap_kib_per_sec.saturating_mul(1024));
            }
        }
    }
    res
}

/// Human status lines for the details pane.
pub fn status_lines(r: &TorrentRules, c: &TorrentCounters, o: &Observation) -> Vec<String> {
    let mut out = Vec::new();
    if r.stalled.enabled {
        if o.finished {
            // not applicable
        } else if c.stalled_latched {
            out.push(format!(
                "Stalled rule fired ({}); re-arms when progress resumes or after a pause",
                r.stalled.action.label()
            ));
        } else {
            let left = r.stalled.after_secs.saturating_sub(c.idle_secs);
            let mut l = format!(
                "No progress for {} (running time); will {} after {} without progress (in {})",
                fmt_secs(c.idle_secs),
                r.stalled.action.label(),
                fmt_secs(r.stalled.after_secs),
                fmt_secs(left)
            );
            if !o.live {
                l.push_str(" — timer stopped while paused/queued");
            }
            out.push(l);
        }
    }
    let s = &r.seeding;
    if s.enabled {
        if !o.finished {
            out.push(format!("Seeding limit applies after completion ({})", seeding_limits_text(s)));
        } else if let Some(t) = c.seeding_fired_unix {
            out.push(format!(
                "Seeding limit reached {} ago ({})",
                fmt_secs((o.now_unix - t).max(0) as u64),
                s.action.label()
            ));
        } else {
            let mut parts = Vec::new();
            if let Some(m) = s.max_seed_secs {
                parts.push(format!("in {}", fmt_secs(m.saturating_sub(c.seeding_secs))));
            }
            if let Some(m) = s.max_uploaded_bytes {
                parts.push(format!("{} left", fmt_bytes(m.saturating_sub(c.uploaded_total))));
            }
            if let Some(m) = s.max_ratio {
                parts.push(format!("ratio {:.2} of {:.2}", ratio(c, o.total_bytes), m.0));
            }
            if !parts.is_empty() {
                out.push(format!("Stops seeding {} → {}", parts.join(" / "), s.action.label()));
            }
        }
    }
    let w = &r.speed_window;
    if w.enabled && o.finished {
        let then = match w.then {
            WindowThen::Cap => format!("cap upload at {} KiB/s", w.cap_kib_per_sec),
            WindowThen::Stop => "stop seeding".to_string(),
        };
        if c.window_done_unix.is_some() {
            out.push(format!("Full-speed window over: {then}"));
        } else {
            let since = c.uploaded_total.saturating_sub(c.uploaded_at_complete);
            let mut parts = Vec::new();
            if let Some(m) = w.full_speed_secs {
                parts.push(fmt_secs(m.saturating_sub(c.seeding_secs)));
            }
            if let Some(m) = w.full_speed_bytes {
                parts.push(fmt_bytes(m.saturating_sub(since)));
            }
            if !parts.is_empty() {
                out.push(format!("Full speed for {} more, then {then}", parts.join(" / ")));
            }
        }
    }
    out
}

fn seeding_limits_text(s: &SeedingLimits) -> String {
    let mut p = Vec::new();
    if let Some(m) = s.max_seed_secs {
        p.push(fmt_secs(m));
    }
    if let Some(m) = s.max_uploaded_bytes {
        p.push(fmt_bytes(m));
    }
    if let Some(m) = s.max_ratio {
        p.push(format!("ratio {:.2}", m.0));
    }
    if p.is_empty() {
        "no limit set".into()
    } else {
        p.join(" / ")
    }
}

pub fn fmt_secs(s: u64) -> String {
    if s < 60 {
        return format!("{s}s");
    }
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    match (d, h, m) {
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

pub fn fmt_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RulesEntry {
    #[serde(default)]
    pub counters: TorrentCounters,
    #[serde(default, rename = "override", skip_serializing_if = "Option::is_none")]
    pub override_: Option<RulesOverride>,
    /// Per-torrent / per-file download order settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_order: Option<crate::download_order::TorrentDownloadOrder>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RulesFile {
    #[serde(default)]
    torrents: HashMap<String, RulesEntry>,
}

/// Persisted counters and overrides, keyed by info hash (hex).
pub struct RulesStore {
    path: Option<PathBuf>,
    entries: Mutex<HashMap<String, RulesEntry>>,
    /// Last live `uploaded_bytes` seen per torrent (in memory; resets with the live state).
    pub(crate) last_uploaded: Mutex<HashMap<String, u64>>,
    /// Torrents whose upload we capped (so we can lift the cap).
    pub(crate) capped: Mutex<HashMap<String, u32>>,
    dirty: std::sync::atomic::AtomicBool,
}

impl RulesStore {
    pub fn load(path: PathBuf) -> Self {
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|b| match serde_json::from_slice::<RulesFile>(&b) {
                Ok(f) => Some(f.torrents),
                Err(e) => {
                    warn!(?path, "error parsing torrent-rules.json: {e:#}");
                    None
                }
            })
            .unwrap_or_default();
        Self {
            path: Some(path),
            entries: Mutex::new(entries),
            last_uploaded: Default::default(),
            capped: Default::default(),
            dirty: Default::default(),
        }
    }

    pub fn get(&self, hash: &str) -> RulesEntry {
        self.entries.lock().get(hash).cloned().unwrap_or_default()
    }

    pub fn with_entry<R>(&self, hash: &str, f: impl FnOnce(&mut RulesEntry) -> R) -> R {
        let mut g = self.entries.lock();
        let r = f(g.entry(hash.to_owned()).or_default());
        self.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
        r
    }

    pub fn set_override(&self, hash: &str, o: Option<RulesOverride>) {
        self.with_entry(hash, |e| e.override_ = o.filter(|o| !o.is_empty()));
        self.save_if_dirty();
    }

    /// Drop everything stored for a torrent (it was forgotten/deleted): re-adding the same
    /// torrent later starts from the global settings again.
    pub fn forget(&self, hash: &str) {
        if self.entries.lock().remove(hash).is_some() {
            self.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
            self.save_if_dirty();
        }
        self.last_uploaded.lock().remove(hash);
        self.capped.lock().remove(hash);
    }

    pub fn retain(&self, present: &std::collections::HashSet<String>) {
        let mut g = self.entries.lock();
        let before = g.len();
        g.retain(|k, _| present.contains(k));
        if g.len() != before {
            self.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.last_uploaded.lock().retain(|k, _| present.contains(k));
        self.capped.lock().retain(|k, _| present.contains(k));
    }

    pub fn save_if_dirty(&self) {
        if !self.dirty.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Some(path) = self.path.as_deref() else { return };
        let file = RulesFile { torrents: self.entries.lock().clone() };
        if let Err(e) = write_atomic(path, &file) {
            warn!(?path, "error saving torrent-rules.json: {e:#}");
            self.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

fn write_atomic(path: &Path, f: &RulesFile) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(f)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `GET /torrents/{id}/rules` response.
#[derive(Debug, Clone, Serialize)]
pub struct TorrentRulesView {
    pub global: TorrentRules,
    #[serde(rename = "override")]
    pub override_: Option<RulesOverride>,
    pub effective: TorrentRules,
    pub counters: TorrentCounters,
    pub ratio: f64,
    pub status: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetRulesOverride {
    #[serde(default, rename = "override")]
    pub override_: Option<RulesOverride>,
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn obs(live: bool, finished: bool, progress: u64, up: u64, now: i64) -> Observation {
        Observation { live, finished, progress_bytes: progress, total_bytes: 1000, uploaded_delta: up, now_unix: now }
    }

    #[test]
    fn stalled_counts_running_time_only() {
        let mut r = TorrentRules::default();
        r.stalled = StalledRule { enabled: true, after_secs: 30, action: RuleAction::Flag };
        let mut c = TorrentCounters::default();
        assert!(evaluate(&r, &mut c, &obs(true, false, 100, 0, 0), 10).firings.is_empty());
        evaluate(&r, &mut c, &obs(true, false, 100, 0, 10), 10);
        // Paused: time doesn't count.
        evaluate(&r, &mut c, &obs(false, false, 100, 0, 20), 100);
        assert_eq!(c.idle_secs, 10);
        evaluate(&r, &mut c, &obs(true, false, 100, 0, 30), 10);
        let res = evaluate(&r, &mut c, &obs(true, false, 100, 0, 40), 10);
        assert_eq!(res.firings.len(), 1);
        assert_eq!(res.firings[0].action, Some(RuleAction::Flag));
        assert_eq!(c.idle_secs, 0);
        // Latched: no repeat firing while still stalled.
        for k in 0..6 {
            let res = evaluate(&r, &mut c, &obs(true, false, 100, 0, 50 + k * 10), 10);
            assert!(res.firings.is_empty());
        }
        assert!(c.stalled_latched);
        // A pause re-arms it.
        evaluate(&r, &mut c, &obs(false, false, 100, 0, 120), 10);
        assert!(!c.stalled_latched);
        c.idle_secs = 0;
        // Progress resets and reports progress.
        evaluate(&r, &mut c, &obs(true, false, 100, 0, 50), 10);
        let res = evaluate(&r, &mut c, &obs(true, false, 150, 0, 60), 10);
        assert!(res.progressed);
        assert_eq!(c.idle_secs, 0);
    }

    #[test]
    fn seeding_limits_fire_once() {
        let mut r = TorrentRules::default();
        r.seeding = SeedingLimits { enabled: true, max_seed_secs: Some(100), max_uploaded_bytes: Some(5000), max_ratio: None, action: RuleAction::Pause };
        let mut c = TorrentCounters::default();
        evaluate(&r, &mut c, &obs(true, true, 1000, 0, 0), 0);
        assert!(c.completed_unix.is_some());
        let res = evaluate(&r, &mut c, &obs(true, true, 1000, 4000, 50), 50);
        assert!(res.firings.is_empty());
        let res = evaluate(&r, &mut c, &obs(true, true, 1000, 1000, 60), 10);
        assert_eq!(res.firings.len(), 1);
        assert!(res.firings[0].reason.contains("uploaded"));
        let res = evaluate(&r, &mut c, &obs(true, true, 1000, 0, 200), 140);
        assert!(res.firings.is_empty());
        // Ratio.
        let mut r2 = TorrentRules::default();
        r2.seeding = SeedingLimits { enabled: true, max_ratio: Some(Ratio(1.5)), ..Default::default() };
        let mut c = TorrentCounters::default();
        assert!(evaluate(&r2, &mut c, &obs(true, true, 1000, 1400, 0), 0).firings.is_empty());
        assert_eq!(evaluate(&r2, &mut c, &obs(true, true, 1000, 100, 1), 1).firings.len(), 1);
    }

    #[test]
    fn speed_window_caps_after_window() {
        let mut r = TorrentRules::default();
        r.speed_window = SpeedWindow { enabled: true, full_speed_secs: Some(60), full_speed_bytes: None, then: WindowThen::Cap, cap_kib_per_sec: 50 };
        let mut c = TorrentCounters::default();
        let res = evaluate(&r, &mut c, &obs(true, true, 1000, 0, 0), 0);
        assert_eq!(res.upload_cap_bps, None);
        let res = evaluate(&r, &mut c, &obs(true, true, 1000, 0, 60), 60);
        assert_eq!(res.upload_cap_bps, Some(50 * 1024));
        assert_eq!(res.firings.len(), 1);
        assert_eq!(res.firings[0].action, None);
        let res = evaluate(&r, &mut c, &obs(true, true, 1000, 0, 70), 10);
        assert!(res.firings.is_empty());
        assert_eq!(res.upload_cap_bps, Some(50 * 1024));
        let lines = status_lines(&r, &c, &obs(true, true, 1000, 0, 70));
        assert!(lines[0].contains("50 KiB/s"));
    }

    #[test]
    fn overrides_and_warnings() {
        let mut g = TorrentRules::default();
        g.seeding.enabled = true;
        g.seeding.action = RuleAction::RemovePolicy;
        assert!(g.delete_warnings(RemovePolicy::KEEP).is_empty());
        assert_eq!(g.delete_warnings(RemovePolicy::DELETE).len(), 1);
        let o = RulesOverride { seeding: Some(SeedingLimits::default()), ..Default::default() };
        let e = g.with_override(&o);
        assert!(!e.seeding.enabled);
        // Seeding can't "finish what's done" or flag.
        let mut s = TorrentRules::default();
        s.seeding.action = RuleAction::Flag;
        assert_eq!(s.sanitize().seeding.action, RuleAction::Pause);
        let mut st = TorrentRules::default();
        st.stalled = StalledRule { enabled: true, after_secs: 60, action: RuleAction::RemoveFinish };
        assert_eq!(st.delete_warnings(RemovePolicy::KEEP).len(), 1);
    }

    #[test]
    fn status_text() {
        let mut r = TorrentRules::default();
        r.seeding = SeedingLimits { enabled: true, max_seed_secs: Some(3 * 3600), max_uploaded_bytes: Some(2 << 30), max_ratio: None, action: RuleAction::Pause };
        let c = TorrentCounters { seeding_secs: 50 * 60, uploaded_total: 800 << 20, completed_unix: Some(0), ..Default::default() };
        let l = status_lines(&r, &c, &obs(true, true, 1000, 0, 0));
        assert_eq!(l[0], "Stops seeding in 2h 10m / 1.2 GiB left → pause");
        assert_eq!(fmt_secs(90000), "1d 1h");
    }
}
