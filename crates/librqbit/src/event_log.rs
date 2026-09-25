//! Rolling, size-capped, persistent log of notable events (repairs, recovery failures,
//! "needs attention", aggregated I/O errors, adoption renames) plus persistent repair
//! counters. Stored next to `preferences.json`:
//!
//! - `events.jsonl` (current segment) and `events.1.jsonl` .. `events.N.jsonl` (older),
//!   one JSON object per line. The total size of all segments never exceeds the
//!   configured cap (default 10 MB): when the current segment would exceed the segment
//!   size (cap / 8, min 32 KiB) it is rotated, and the oldest segments are deleted until
//!   the rotated ones fit in `cap - segment`.
//! - `repair-counters.json`: counters since the last reset.
//!
//! Identical I/O errors (same torrent, file, operation and error) are aggregated: the
//! first one in a 60 s window is written immediately, the rest are counted and written
//! as one record (`count`) when the window ends.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::{BufRead, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Run blocking event-log work on its own short-lived thread. Deliberately not
/// `spawn_blocking`: tokio's blocking pool can be saturated by disk I/O (e.g. many
/// torrents checking at startup), which made event queries take 20+ seconds.
pub async fn off_runtime<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> anyhow::Result<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("rqbit-events".into())
        .spawn(move || {
            let _ = tx.send(f());
        })?;
    Ok(rx.await?)
}

pub const DEFAULT_CAP_BYTES: u64 = 10 * 1024 * 1024;
pub const MIN_CAP_BYTES: u64 = 256 * 1024;
pub const MAX_CAP_BYTES: u64 = 1024 * 1024 * 1024;
const MIN_SEGMENT_BYTES: u64 = 32 * 1024;
/// A single record is truncated to stay below this.
const MAX_LINE_BYTES: usize = 16 * 1024;
const INDEX_CAP: usize = 50_000;
pub const AGGREGATION_WINDOW: Duration = Duration::from_secs(60);
const MAX_AGG_KEYS: usize = 2_000;

const CURRENT: &str = "events.jsonl";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "info" => Some(Self::Info),
            "warning" | "warn" => Some(Self::Warning),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

pub mod kind {
    pub const REPAIR_RUN: &str = "repair_run";
    pub const REPAIR_FILE: &str = "repair_file";
    pub const PIECE_RETRY: &str = "piece_retry_scheduled";
    pub const NEEDS_ATTENTION: &str = "needs_attention";
    pub const IO_ERROR: &str = "io_error";
    pub const DAMAGE_DETECTED: &str = "damage_detected";
    pub const ADOPTION: &str = "adoption";
    pub const TORRENT_ERROR: &str = "torrent_error";
    pub const RECHECK: &str = "recheck";
}

fn one() -> u64 {
    1
}

fn is_one(v: &u64) -> bool {
    *v == 1
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EventRecord {
    pub seq: u64,
    pub time: String,
    pub kind: String,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_id: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
    /// How many occurrences this record stands for (aggregated I/O errors).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub count: u64,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

#[derive(Clone, Debug, Default)]
pub struct TorrentRef {
    /// None when not known yet (e.g. while adding).
    pub id: Option<usize>,
    pub info_hash: String,
    pub name: Option<String>,
}

/// An event before it gets a sequence number / timestamp.
#[derive(Clone, Debug)]
pub struct NewEvent {
    pub kind: &'static str,
    pub severity: Severity,
    pub torrent: Option<TorrentRef>,
    pub file_id: Option<usize>,
    pub path: Option<String>,
    pub message: String,
    pub count: u64,
    pub details: serde_json::Value,
    /// Original timestamp (migrated records); now when None.
    pub time: Option<String>,
}

impl NewEvent {
    pub fn new(kind: &'static str, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            kind,
            severity,
            torrent: None,
            file_id: None,
            path: None,
            message: message.into(),
            count: 1,
            details: serde_json::Value::Null,
            time: None,
        }
    }
    pub fn torrent(mut self, t: TorrentRef) -> Self {
        self.torrent = Some(t);
        self
    }
    pub fn file(mut self, file_id: Option<usize>, path: Option<String>) -> Self {
        self.file_id = file_id;
        self.path = path;
        self
    }
    pub fn details(mut self, d: serde_json::Value) -> Self {
        self.details = d;
        self
    }
}

// ---------------------------------------------------------------------------------------
// Rotation (pure file logic, unit-tested)
// ---------------------------------------------------------------------------------------

pub fn segment_size(cap: u64) -> u64 {
    (cap / 8).max(MIN_SEGMENT_BYTES).min(cap.max(1))
}

pub fn clamp_cap(cap: u64) -> u64 {
    cap.clamp(MIN_CAP_BYTES, MAX_CAP_BYTES)
}

fn rotated_name(i: usize) -> String {
    format!("events.{i}.jsonl")
}

/// Current segment + rotated segments in one directory.
pub struct Rotator {
    dir: PathBuf,
    cap: u64,
    cur_size: u64,
    /// Sizes of rotated segments; index 0 = `events.1.jsonl` (newest).
    rotated: Vec<u64>,
}

impl Rotator {
    pub fn open(dir: &Path, cap: u64) -> std::io::Result<Self> {
        let mut r = Self::open_raw(dir, cap)?;
        r.enforce()?;
        Ok(r)
    }

    /// Open without enforcing the cap yet (call `enforce`).
    pub fn open_raw(dir: &Path, cap: u64) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let cur_size = std::fs::metadata(dir.join(CURRENT))
            .map(|m| m.len())
            .unwrap_or(0);
        let mut rotated = Vec::new();
        for i in 1.. {
            match std::fs::metadata(dir.join(rotated_name(i))) {
                Ok(m) => rotated.push(m.len()),
                Err(_) => break,
            }
        }
        // Stray segments past a gap (shouldn't happen) are removed so they can't pile up.
        let mut i = rotated.len() + 2;
        while dir.join(rotated_name(i)).exists() && i < 10_000 {
            let _ = std::fs::remove_file(dir.join(rotated_name(i)));
            i += 1;
        }
        Ok(Self {
            dir: dir.to_owned(),
            cap,
            cur_size,
            rotated,
        })
    }

    pub fn total_size(&self) -> u64 {
        self.cur_size + self.rotated.iter().sum::<u64>()
    }

    pub fn segments(&self) -> usize {
        1 + self.rotated.len()
    }

    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// Paths newest first (current first).
    pub fn paths_newest_first(&self) -> Vec<PathBuf> {
        let mut v = vec![self.dir.join(CURRENT)];
        v.extend((1..=self.rotated.len()).map(|i| self.dir.join(rotated_name(i))));
        v
    }

    pub fn set_cap(&mut self, cap: u64) -> std::io::Result<()> {
        if cap != self.cap {
            self.cap = cap;
            self.enforce()?;
        }
        Ok(())
    }

    pub fn enforce(&mut self) -> std::io::Result<()> {
        let seg = segment_size(self.cap);
        if self.cur_size > seg {
            self.rotate()?;
        } else {
            self.drop_oldest(seg)?;
        }
        Ok(())
    }

    fn drop_oldest(&mut self, seg: u64) -> std::io::Result<()> {
        let budget = self.cap.saturating_sub(seg);
        while !self.rotated.is_empty() && self.rotated.iter().sum::<u64>() > budget {
            let n = self.rotated.len();
            match std::fs::remove_file(self.dir.join(rotated_name(n))) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            self.rotated.pop();
        }
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        let seg = segment_size(self.cap);
        let budget = self.cap.saturating_sub(seg);
        let cur = self.dir.join(CURRENT);
        if self.cur_size > budget {
            // The current segment alone doesn't fit (cap shrank): keep only its newest
            // lines, then rotate it (older segments are dropped below).
            self.cur_size = trim_to_tail(&cur, budget)?;
        }
        {
            // Drop what won't fit after this rotation first (fewer renames).
            while !self.rotated.is_empty()
                && self.rotated.iter().sum::<u64>() + self.cur_size > budget
            {
                let n = self.rotated.len();
                let _ = std::fs::remove_file(self.dir.join(rotated_name(n)));
                self.rotated.pop();
            }
            for i in (1..=self.rotated.len()).rev() {
                std::fs::rename(
                    self.dir.join(rotated_name(i)),
                    self.dir.join(rotated_name(i + 1)),
                )?;
            }
            match std::fs::rename(&cur, self.dir.join(rotated_name(1))) {
                Ok(()) => self.rotated.insert(0, self.cur_size),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        self.cur_size = 0;
        self.drop_oldest(seg)
    }

    /// Append one line (must end with '\n'), rotating first when needed.
    pub fn append(&mut self, line: &[u8]) -> std::io::Result<()> {
        let seg = segment_size(self.cap);
        if self.cur_size > 0 && self.cur_size + line.len() as u64 > seg {
            self.rotate()?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(CURRENT))?;
        f.write_all(line)?;
        self.cur_size += line.len() as u64;
        Ok(())
    }
}

/// Keep only the last `keep` bytes of a line-oriented file (starting at a line boundary).
fn trim_to_tail(path: &Path, keep: u64) -> std::io::Result<u64> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len <= keep {
        return Ok(len);
    }
    f.seek(SeekFrom::Start(len - keep))?;
    let mut buf = Vec::with_capacity(keep as usize);
    f.read_to_end(&mut buf)?;
    let start = buf.iter().position(|b| *b == b'\n').map(|p| p + 1).unwrap_or(buf.len());
    let tail = &buf[start..];
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, tail)?;
    std::fs::rename(&tmp, path)?;
    Ok(tail.len() as u64)
}

// ---------------------------------------------------------------------------------------
// I/O error aggregation (pure, unit-tested)
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AggKey {
    pub torrent: Option<String>,
    pub file_id: Option<usize>,
    pub op: String,
    pub error: String,
}

struct AggEntry {
    window_start: Instant,
    suppressed: u64,
    template: NewEvent,
}

/// Rate limiter for repeated identical errors.
pub struct Aggregator {
    window: Duration,
    entries: HashMap<AggKey, AggEntry>,
}

impl Aggregator {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            entries: HashMap::new(),
        }
    }

    /// Offer an occurrence. Returns the event to write now (first in its window), or None
    /// when it was counted into an open window.
    pub fn offer(&mut self, key: AggKey, ev: NewEvent, now: Instant) -> (Option<NewEvent>, Vec<NewEvent>) {
        let mut flushed = Vec::new();
        if let Some(e) = self.entries.get_mut(&key) {
            if now.duration_since(e.window_start) < self.window {
                e.suppressed += 1;
                return (None, flushed);
            }
            // Window over: emit what was suppressed, start a new window with this one.
            if let Some(f) = self.entries.remove(&key).and_then(|e| self.summary(e)) {
                flushed.push(f);
            }
        }
        if self.entries.len() >= MAX_AGG_KEYS {
            flushed.extend(self.flush(now, true));
        }
        self.entries.insert(
            key,
            AggEntry {
                window_start: now,
                suppressed: 0,
                template: ev.clone(),
            },
        );
        (Some(ev), flushed)
    }

    fn summary(&self, e: AggEntry) -> Option<NewEvent> {
        if e.suppressed == 0 {
            return None;
        }
        let mut ev = e.template;
        ev.count = e.suppressed;
        ev.message = format!(
            "{} (repeated {} more time{} within {}s)",
            ev.message,
            e.suppressed,
            if e.suppressed == 1 { "" } else { "s" },
            self.window.as_secs()
        );
        Some(ev)
    }

    /// Close windows that ended (all of them when `force`). Returns aggregated records.
    pub fn flush(&mut self, now: Instant, force: bool) -> Vec<NewEvent> {
        let window = self.window;
        let expired: Vec<AggKey> = self
            .entries
            .iter()
            .filter(|(_, e)| force || now.duration_since(e.window_start) >= window)
            .map(|(k, _)| k.clone())
            .collect();
        let mut out = Vec::new();
        for k in expired {
            if let Some(e) = self.entries.remove(&k) {
                out.extend(self.summary(e));
            }
        }
        out
    }

    pub fn open_windows(&self) -> usize {
        self.entries.len()
    }
}

// ---------------------------------------------------------------------------------------
// Counters
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct RepairCounters {
    pub since: String,
    #[serde(default)]
    pub auto_repairs: u64,
    #[serde(default)]
    pub manual_repairs: u64,
    #[serde(default)]
    pub repair_failures: u64,
    #[serde(default)]
    pub files_repaired: u64,
    #[serde(default)]
    pub bytes_unreadable: u64,
    #[serde(default)]
    pub bytes_zeroed: u64,
    /// Bytes to download again (requeued pieces x piece length).
    #[serde(default)]
    pub bytes_redownload: u64,
    #[serde(default)]
    pub pieces_requeued: u64,
    /// Automatic recovery gave up (piece retries or automatic repairs): needs attention.
    #[serde(default)]
    pub give_ups: u64,
    #[serde(default)]
    pub piece_retries: u64,
    #[serde(default)]
    pub io_errors: u64,
    /// Repair runs per torrent (info hash).
    #[serde(default)]
    pub per_torrent: BTreeMap<String, u64>,
}

impl RepairCounters {
    fn fresh() -> Self {
        Self {
            since: crate::adopt::rfc3339_now(),
            ..Default::default()
        }
    }
}

/// Result of a finished repair run, for counters.
#[derive(Clone, Debug, Default)]
pub struct RepairRunStats {
    pub auto: bool,
    pub failed: bool,
    pub files_repaired: u64,
    pub bytes_unreadable: u64,
    pub bytes_zeroed: u64,
    pub pieces_requeued: u64,
    pub bytes_redownload: u64,
}

// ---------------------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------------------

#[derive(Deserialize, Default, Clone, Debug)]
pub struct EventQuery {
    /// Comma-separated kinds.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub torrent_id: Option<usize>,
    #[serde(default)]
    pub info_hash: Option<String>,
    /// Minimum severity (info < warning < error).
    #[serde(default)]
    pub severity: Option<String>,
    /// RFC 3339 UTC timestamp (e.g. 2026-09-25T00:00:00Z).
    #[serde(default)]
    pub since: Option<String>,
    /// Only events with seq > since_seq.
    #[serde(default)]
    pub since_seq: Option<u64>,
    /// Pagination: only events with seq < before_seq.
    #[serde(default)]
    pub before_seq: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Serialize, Debug)]
pub struct EventPage {
    pub events: Vec<EventRecord>,
    /// Pass as `before_seq` to get the next (older) page.
    pub next_before_seq: Option<u64>,
    pub latest_seq: u64,
}

#[derive(Serialize, Debug, Default, Clone, Copy)]
pub struct UnseenCounts {
    pub repairs: u64,
    pub errors: u64,
    pub warnings: u64,
    pub total: u64,
}

#[derive(Serialize, Debug)]
pub struct EventSummary {
    pub counters: RepairCounters,
    pub latest_seq: u64,
    pub unseen: UnseenCounts,
    pub log_bytes: u64,
    pub log_cap_bytes: u64,
    pub log_segments: usize,
}

struct Filter {
    kinds: Option<Vec<String>>,
    torrent_id: Option<usize>,
    info_hash: Option<String>,
    min_severity: Option<Severity>,
    since: Option<String>,
    since_seq: Option<u64>,
    before_seq: Option<u64>,
}

impl Filter {
    fn from_query(q: &EventQuery) -> Self {
        Self {
            kinds: q
                .kind
                .as_ref()
                .map(|k| k.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect::<Vec<_>>())
                .filter(|v| !v.is_empty()),
            torrent_id: q.torrent_id,
            info_hash: q.info_hash.as_ref().map(|s| s.to_ascii_lowercase()),
            min_severity: q.severity.as_deref().and_then(Severity::parse),
            since: q.since.clone(),
            since_seq: q.since_seq,
            before_seq: q.before_seq,
        }
    }

    fn matches(&self, r: &EventRecord) -> bool {
        if let Some(b) = self.before_seq {
            if r.seq >= b {
                return false;
            }
        }
        if let Some(s) = self.since_seq {
            if r.seq <= s {
                return false;
            }
        }
        if let Some(k) = &self.kinds {
            if !k.iter().any(|k| *k == r.kind) {
                return false;
            }
        }
        // Torrent ids can change across restarts; prefer the info hash when both given.
        if let Some(ih) = &self.info_hash {
            if r.info_hash.as_deref() != Some(ih.as_str()) {
                return false;
            }
        } else if let Some(t) = self.torrent_id {
            if r.torrent_id != Some(t) {
                return false;
            }
        }
        if let Some(s) = self.min_severity {
            if r.severity < s {
                return false;
            }
        }
        if let Some(since) = &self.since {
            if r.time.as_str() < since.as_str() {
                return false;
            }
        }
        true
    }
}

fn read_lines_rev(path: &Path) -> Vec<EventRecord> {
    let Ok(f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut v: Vec<EventRecord> = std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect();
    v.reverse();
    v
}

/// Filter + paginate over segment files (newest first).
pub fn query_files(paths: &[PathBuf], q: &EventQuery) -> (Vec<EventRecord>, Option<u64>) {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let f = Filter::from_query(q);
    let mut out: Vec<EventRecord> = Vec::new();
    let mut more = false;
    'files: for p in paths {
        let mut recs = read_lines_rev(p);
        // Lines are appended in seq order, but be robust to manual edits.
        recs.sort_by(|a, b| b.seq.cmp(&a.seq));
        for r in recs {
            if f.since_seq.is_some_and(|s| r.seq <= s) {
                break 'files;
            }
            if f.matches(&r) {
                if out.len() == limit {
                    more = true;
                    break 'files;
                }
                out.push(r);
            }
        }
    }
    let next = if more { out.last().map(|r| r.seq) } else { None };
    (out, next)
}

fn last_seq_in(path: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(64 * 1024);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).ok()?;
    buf.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<EventRecord>(l).ok())
        .map(|r| r.seq)
        .max()
}

// ---------------------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct IndexEntry {
    seq: u64,
    severity: Severity,
    is_repair: bool,
}

struct Inner {
    rot: Option<Rotator>,
    next_seq: u64,
    index: VecDeque<IndexEntry>,
    agg: Aggregator,
    counters: RepairCounters,
    counters_dirty: bool,
}

pub struct EventLog {
    dir: PathBuf,
    counters_path: PathBuf,
    cap: AtomicU64,
    inner: Mutex<Inner>,
}

fn counters_path(dir: &Path) -> PathBuf {
    dir.join("repair-counters.json")
}

impl EventLog {
    /// Open (or create) the log in `dir`. Never fails: problems are logged and the log
    /// degrades to in-memory counters.
    pub fn open(dir: &Path, cap: u64) -> Self {
        let cap = clamp_cap(cap);
        let rot = match Rotator::open_raw(dir, cap) {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(?dir, "event log disabled: {e:#}");
                None
            }
        };
        let mut next_seq = 1;
        let mut rot = rot;
        if let Some(r) = &mut rot {
            for p in r.paths_newest_first() {
                if let Some(s) = last_seq_in(&p) {
                    next_seq = s + 1;
                    break;
                }
            }
            // Apply the cap after recovering the sequence number, so shrinking the cap
            // never makes sequence numbers go backwards.
            if let Err(e) = r.enforce() {
                warn!(?dir, "event log: error enforcing size cap: {e:#}");
            }
        }
        let cpath = counters_path(dir);
        let counters_exist = cpath.exists();
        let counters = std::fs::read(&cpath)
            .ok()
            .and_then(|b| serde_json::from_slice::<RepairCounters>(&b).ok())
            .unwrap_or_else(RepairCounters::fresh);
        let log = Self {
            dir: dir.to_owned(),
            counters_path: cpath,
            cap: AtomicU64::new(cap),
            inner: Mutex::new(Inner {
                rot,
                next_seq,
                index: VecDeque::new(),
                agg: Aggregator::new(AGGREGATION_WINDOW),
                counters,
                counters_dirty: !counters_exist,
            }),
        };
        log.migrate_legacy(!counters_exist);
        log.rebuild_index();
        log.save_counters_if_dirty();
        log
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn cap(&self) -> u64 {
        self.cap.load(Ordering::Relaxed)
    }

    pub fn set_cap(&self, cap: u64) {
        let cap = clamp_cap(cap);
        if self.cap.swap(cap, Ordering::Relaxed) == cap {
            return;
        }
        let mut g = self.inner.lock();
        if let Some(r) = g.rot.as_mut() {
            if let Err(e) = r.set_cap(cap) {
                warn!("event log: error applying new size cap: {e:#}");
            }
        }
        info!(cap, "event log size cap updated");
    }

    fn rebuild_index(&self) {
        let paths = match self.inner.lock().rot.as_ref() {
            Some(r) => r.paths_newest_first(),
            None => return,
        };
        let mut entries: Vec<IndexEntry> = Vec::new();
        for p in paths {
            for r in read_lines_rev(&p) {
                entries.push(IndexEntry {
                    seq: r.seq,
                    severity: r.severity,
                    is_repair: r.kind == kind::REPAIR_RUN,
                });
                if entries.len() >= INDEX_CAP {
                    break;
                }
            }
            if entries.len() >= INDEX_CAP {
                break;
            }
        }
        entries.sort_by_key(|e| e.seq);
        self.inner.lock().index = entries.into_iter().collect();
    }

    fn write_locked(g: &mut Inner, ev: NewEvent) -> u64 {
        let seq = g.next_seq;
        g.next_seq += 1;
        let t = ev.torrent;
        let mut rec = EventRecord {
            seq,
            time: ev.time.unwrap_or_else(crate::adopt::rfc3339_now),
            kind: ev.kind.to_owned(),
            severity: ev.severity,
            torrent_id: t.as_ref().and_then(|t| t.id),
            info_hash: t.as_ref().map(|t| t.info_hash.clone()),
            torrent_name: t.and_then(|t| t.name),
            file_id: ev.file_id,
            path: ev.path,
            message: ev.message,
            count: ev.count.max(1),
            details: ev.details,
        };
        let mut line = serde_json::to_string(&rec).unwrap_or_default();
        if line.len() >= MAX_LINE_BYTES {
            rec.details = serde_json::json!({"truncated": true});
            rec.message = rec.message.chars().take(2000).collect();
            line = serde_json::to_string(&rec).unwrap_or_default();
        }
        line.push('\n');
        if let Some(r) = g.rot.as_mut() {
            if let Err(e) = r.append(line.as_bytes()) {
                warn!("event log: write failed: {e:#}");
            }
        }
        g.index.push_back(IndexEntry {
            seq,
            severity: rec.severity,
            is_repair: rec.kind == kind::REPAIR_RUN,
        });
        while g.index.len() > INDEX_CAP {
            g.index.pop_front();
        }
        seq
    }

    /// Append an event.
    pub fn emit(&self, ev: NewEvent) -> u64 {
        Self::write_locked(&mut self.inner.lock(), ev)
    }

    /// Record an I/O error, aggregated per (torrent, file, op, error) per minute.
    pub fn io_error(&self, key: AggKey, ev: NewEvent) {
        self.io_error_at(key, ev, Instant::now())
    }

    pub fn io_error_at(&self, key: AggKey, ev: NewEvent, now: Instant) {
        self.aggregated_at(key, ev, now, true)
    }

    /// Append an event, aggregating identical ones (same key) per minute.
    pub fn aggregated(&self, key: AggKey, ev: NewEvent) {
        self.aggregated_at(key, ev, Instant::now(), false)
    }

    fn aggregated_at(&self, key: AggKey, ev: NewEvent, now: Instant, count_io_error: bool) {
        let mut g = self.inner.lock();
        if count_io_error {
            g.counters.io_errors += 1;
            g.counters_dirty = true;
        }
        let (first, flushed) = g.agg.offer(key, ev, now);
        for f in flushed.into_iter().chain(first) {
            Self::write_locked(&mut g, f);
        }
    }

    /// Periodic: write aggregated records for ended windows, persist counters.
    pub fn tick(&self) {
        self.tick_at(Instant::now(), false)
    }

    pub fn tick_at(&self, now: Instant, force: bool) {
        {
            let mut g = self.inner.lock();
            for f in g.agg.flush(now, force) {
                Self::write_locked(&mut g, f);
            }
        }
        self.save_counters_if_dirty();
    }

    fn save_counters_if_dirty(&self) {
        let data = {
            let mut g = self.inner.lock();
            if !g.counters_dirty {
                return;
            }
            g.counters_dirty = false;
            serde_json::to_vec_pretty(&g.counters).unwrap_or_default()
        };
        let tmp = self.counters_path.with_extension("json.tmp");
        let res = std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, &self.counters_path));
        if let Err(e) = res {
            warn!(path=?self.counters_path, "error saving repair counters: {e:#}");
        }
    }

    pub fn record_repair_run(&self, info_hash: &str, s: &RepairRunStats) {
        {
            let mut g = self.inner.lock();
            let c = &mut g.counters;
            if s.auto {
                c.auto_repairs += 1;
            } else {
                c.manual_repairs += 1;
            }
            if s.failed {
                c.repair_failures += 1;
            }
            c.files_repaired += s.files_repaired;
            c.bytes_unreadable += s.bytes_unreadable;
            c.bytes_zeroed += s.bytes_zeroed;
            c.pieces_requeued += s.pieces_requeued;
            c.bytes_redownload += s.bytes_redownload;
            *c.per_torrent.entry(info_hash.to_owned()).or_default() += 1;
            g.counters_dirty = true;
        }
        self.save_counters_if_dirty();
    }

    pub fn record_give_up(&self) {
        {
            let mut g = self.inner.lock();
            g.counters.give_ups += 1;
            g.counters_dirty = true;
        }
        self.save_counters_if_dirty();
    }

    pub fn record_piece_retry(&self) {
        let mut g = self.inner.lock();
        g.counters.piece_retries += 1;
        g.counters_dirty = true;
    }

    pub fn repair_count(&self, info_hash: &str) -> u64 {
        self.inner
            .lock()
            .counters
            .per_torrent
            .get(info_hash)
            .copied()
            .unwrap_or(0)
    }

    pub fn counters(&self) -> RepairCounters {
        self.inner.lock().counters.clone()
    }

    pub fn reset_counters(&self) -> RepairCounters {
        let c = {
            let mut g = self.inner.lock();
            g.counters = RepairCounters::fresh();
            g.counters_dirty = true;
            g.counters.clone()
        };
        self.save_counters_if_dirty();
        info!("repair counters reset");
        c
    }

    pub fn latest_seq(&self) -> u64 {
        self.inner.lock().next_seq.saturating_sub(1)
    }

    pub fn summary(&self, since_seq: Option<u64>) -> EventSummary {
        let g = self.inner.lock();
        let mut unseen = UnseenCounts::default();
        let since = since_seq.unwrap_or(u64::MAX);
        for e in g.index.iter().rev() {
            if e.seq <= since {
                break;
            }
            unseen.total += 1;
            if e.is_repair {
                unseen.repairs += 1;
            }
            match e.severity {
                Severity::Error => unseen.errors += 1,
                Severity::Warning => unseen.warnings += 1,
                Severity::Info => {}
            }
        }
        EventSummary {
            counters: g.counters.clone(),
            latest_seq: g.next_seq.saturating_sub(1),
            unseen,
            log_bytes: g.rot.as_ref().map(|r| r.total_size()).unwrap_or(0),
            log_cap_bytes: self.cap(),
            log_segments: g.rot.as_ref().map(|r| r.segments()).unwrap_or(0),
        }
    }

    pub fn segment_paths(&self) -> Vec<PathBuf> {
        self.inner
            .lock()
            .rot
            .as_ref()
            .map(|r| r.paths_newest_first())
            .unwrap_or_default()
    }

    /// Blocking: reads the segment files.
    pub fn query(&self, q: &EventQuery) -> EventPage {
        let paths = self.segment_paths();
        let (events, next_before_seq) = query_files(&paths, q);
        EventPage {
            events,
            next_before_seq,
            latest_seq: self.latest_seq(),
        }
    }

    /// One-time import of the old `repair-log.jsonl` / `adopt-log.jsonl` files (renamed to
    /// `*.migrated` afterwards).
    fn migrate_legacy(&self, backfill_counters: bool) {
        let repair = self.dir.join("repair-log.jsonl");
        let adopt = self.dir.join("adopt-log.jsonl");
        let mut events: Vec<NewEvent> = Vec::new();
        let mut runs: Vec<(String, RepairRunStats)> = Vec::new();
        if let Ok(s) = std::fs::read_to_string(&repair) {
            let (ev, r) = parse_legacy_repair_log(&s);
            events.extend(ev);
            runs = r;
        }
        if let Ok(s) = std::fs::read_to_string(&adopt) {
            events.extend(parse_legacy_adopt_log(&s));
        }
        if events.is_empty() && !repair.exists() && !adopt.exists() {
            return;
        }
        events.sort_by(|a, b| a.time.cmp(&b.time));
        let n = events.len();
        {
            let mut g = self.inner.lock();
            for e in events {
                Self::write_locked(&mut g, e);
            }
        }
        if backfill_counters {
            for (ih, r) in &runs {
                self.record_repair_run(ih, r);
            }
            // Counters cover the imported history.
            let first = {
                let g = self.inner.lock();
                g.index.front().map(|e| e.seq)
            };
            if let Some(first) = first {
                let q = EventQuery {
                    before_seq: Some(first + 1),
                    limit: Some(1),
                    ..Default::default()
                };
                let (recs, _) = query_files(&self.segment_paths(), &q);
                if let Some(r) = recs.first() {
                    let mut g = self.inner.lock();
                    if r.time < g.counters.since {
                        g.counters.since = r.time.clone();
                        g.counters_dirty = true;
                    }
                }
            }
        }
        for p in [&repair, &adopt] {
            if p.exists() {
                let mut to = p.clone().into_os_string();
                to.push(".migrated");
                if let Err(e) = std::fs::rename(p, &to) {
                    warn!(path=?p, "event log: could not rename migrated legacy log: {e:#}");
                }
            }
        }
        info!(events = n, repair_runs = runs.len(), "event log: migrated legacy repair/adopt logs");
    }
}

/// Legacy repair log -> events + repair runs (consecutive records of the same torrent form
/// one run; all legacy runs were manual).
pub fn parse_legacy_repair_log(s: &str) -> (Vec<NewEvent>, Vec<(String, RepairRunStats)>) {
    let mut events = Vec::new();
    let mut runs: Vec<(String, RepairRunStats)> = Vec::new();
    for line in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let get_u = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        let ih = v.get("info_hash").and_then(|x| x.as_str()).unwrap_or("").to_owned();
        let method = v.get("method").and_then(|x| x.as_str()).unwrap_or("").to_owned();
        let path = v.get("path").and_then(|x| x.as_str()).map(|s| s.to_owned());
        let failed = method == "failed";
        let repaired = method == "punch_hole" || method == "copy_replace";
        let pieces = v
            .get("pieces_affected")
            .and_then(|x| x.as_array())
            .map(|a| a.len() as u64)
            .unwrap_or(0);
        let fname = path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut ev = NewEvent::new(
            kind::REPAIR_FILE,
            if failed { Severity::Error } else { Severity::Info },
            if failed {
                format!("Repair failed: {fname}")
            } else {
                format!(
                    "Repaired {fname} ({method}): {} bytes unreadable, {} pieces to re-download",
                    get_u("bytes_unreadable"),
                    pieces
                )
            },
        )
        .file(v.get("file_id").and_then(|x| x.as_u64()).map(|x| x as usize), path);
        ev.torrent = Some(TorrentRef {
            id: v.get("torrent_id").and_then(|x| x.as_u64()).map(|x| x as usize),
            info_hash: ih.clone(),
            name: None,
        });
        let mut d = v.clone();
        if let Some(o) = d.as_object_mut() {
            o.insert("legacy".into(), serde_json::Value::Bool(true));
            o.insert("auto".into(), serde_json::Value::Bool(false));
        }
        ev.details = d;
        ev.time = v.get("time").and_then(|x| x.as_str()).map(|s| s.to_owned());
        events.push(ev);

        if runs.last().map(|(h, _)| h != &ih).unwrap_or(true) {
            runs.push((ih.clone(), RepairRunStats::default()));
        }
        let r = &mut runs.last_mut().unwrap().1;
        r.failed |= failed;
        if repaired {
            r.files_repaired += 1;
        }
        r.bytes_unreadable += get_u("bytes_unreadable");
        r.bytes_zeroed += get_u("bytes_zeroed");
        r.pieces_requeued += pieces;
    }
    (events, runs)
}

pub fn parse_legacy_adopt_log(s: &str) -> Vec<NewEvent> {
    let mut out = Vec::new();
    for line in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let st = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.to_owned());
        let from = st("from").unwrap_or_default();
        let to = st("to").unwrap_or_default();
        let fname = |p: &str| {
            Path::new(p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut ev = NewEvent::new(
            kind::ADOPTION,
            Severity::Info,
            format!("Adopted partial file {} → {}", fname(&from), fname(&to)),
        )
        .file(None, Some(to.clone()));
        ev.torrent = st("info_hash").map(|ih| TorrentRef {
            id: None,
            info_hash: ih,
            name: st("name"),
        });
        let mut d = v.clone();
        if let Some(o) = d.as_object_mut() {
            o.insert("legacy".into(), serde_json::Value::Bool(true));
        }
        ev.details = d;
        ev.time = st("ts");
        out.push(ev);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: usize) -> Vec<u8> {
        let mut v = vec![b'x'; n - 1];
        v.push(b'\n');
        v
    }

    fn dir_total(dir: &Path) -> u64 {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("events"))
            .map(|e| e.metadata().unwrap().len())
            .sum()
    }

    #[test]
    fn rotation_never_exceeds_cap_and_drops_oldest() {
        let d = tempfile::tempdir().unwrap();
        let cap = 256 * 1024; // segment = 32 KiB
        let mut r = Rotator::open(d.path(), cap).unwrap();
        assert_eq!(segment_size(cap), 32 * 1024);
        for i in 0..20_000 {
            r.append(&line(100 + (i % 900))).unwrap();
            assert!(r.total_size() <= cap, "total {} > cap", r.total_size());
        }
        assert_eq!(dir_total(d.path()), r.total_size());
        assert!(dir_total(d.path()) <= cap);
        // Kept most of the budget (at least cap - 2 segments).
        assert!(r.total_size() >= cap - 2 * segment_size(cap));
        // No files beyond the tracked segments.
        let n = r.segments();
        assert!(!d.path().join(rotated_name(n)).exists());
        assert!(d.path().join(rotated_name(n - 1)).exists());
    }

    #[test]
    fn rotation_keeps_newest_records() {
        let d = tempfile::tempdir().unwrap();
        let log = EventLog::open(d.path(), MIN_CAP_BYTES);
        for i in 0..20_000u64 {
            log.emit(NewEvent::new(kind::IO_ERROR, Severity::Error, format!("event number {i} {}", "p".repeat(100))));
        }
        let s = log.summary(None);
        assert!(s.log_bytes <= MIN_CAP_BYTES);
        assert_eq!(s.latest_seq, 20_000);
        let page = log.query(&EventQuery {
            limit: Some(5),
            ..Default::default()
        });
        let seqs: Vec<u64> = page.events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![20_000, 19_999, 19_998, 19_997, 19_996]);
        // Oldest were dropped.
        let all = query_files(&log.segment_paths(), &EventQuery { limit: Some(1000), before_seq: Some(2), ..Default::default() });
        assert!(all.0.is_empty());
        // Pagination continues from next_before_seq.
        let p2 = log.query(&EventQuery {
            limit: Some(3),
            before_seq: page.next_before_seq,
            ..Default::default()
        });
        assert_eq!(p2.events[0].seq, 19_995);
    }

    #[test]
    fn reopen_recovers_seq_and_shrinking_cap_enforced() {
        let d = tempfile::tempdir().unwrap();
        {
            let log = EventLog::open(d.path(), 4 * 1024 * 1024);
            for i in 0..30_000u64 {
                log.emit(NewEvent::new(kind::REPAIR_RUN, Severity::Info, format!("run {i} {}", "q".repeat(80))));
            }
            assert!(dir_total(d.path()) > MIN_CAP_BYTES);
        }
        let log = EventLog::open(d.path(), MIN_CAP_BYTES);
        assert!(dir_total(d.path()) <= MIN_CAP_BYTES);
        assert_eq!(log.latest_seq(), 30_000);
        let seq = log.emit(NewEvent::new(kind::REPAIR_RUN, Severity::Info, "next"));
        assert_eq!(seq, 30_001);
        // Unseen counts from the index.
        let s = log.summary(Some(29_990));
        assert_eq!(s.unseen.total, 11);
        assert_eq!(s.unseen.repairs, 11);
        // Growing / shrinking the cap at runtime.
        log.set_cap(MIN_CAP_BYTES * 4);
        log.set_cap(MIN_CAP_BYTES);
        assert!(dir_total(d.path()) <= MIN_CAP_BYTES);
    }

    fn key(file: usize) -> AggKey {
        AggKey {
            torrent: Some("abc".into()),
            file_id: Some(file),
            op: "write".into(),
            error: "Input/output error (os error 5)".into(),
        }
    }

    #[test]
    fn aggregation_window_counts_and_flushes() {
        let mut a = Aggregator::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let ev = NewEvent::new(kind::IO_ERROR, Severity::Error, "I/O write error");
        // First is emitted immediately.
        let (first, fl) = a.offer(key(1), ev.clone(), t0);
        assert!(first.is_some() && fl.is_empty());
        // 499 more in the same minute are suppressed.
        for i in 1..500 {
            let (f, fl) = a.offer(key(1), ev.clone(), t0 + Duration::from_millis(i * 100));
            assert!(f.is_none() && fl.is_empty());
        }
        // A different file is its own stream.
        assert!(a.offer(key(2), ev.clone(), t0 + Duration::from_secs(1)).0.is_some());
        // Nothing to flush before the window ends.
        assert!(a.flush(t0 + Duration::from_secs(59), false).is_empty());
        let out = a.flush(t0 + Duration::from_secs(61), false);
        assert_eq!(out.len(), 1, "file 2 had no repeats");
        assert_eq!(out[0].count, 499);
        assert!(out[0].message.contains("repeated 499 more times"));
        assert_eq!(a.open_windows(), 0);
        // After the window, the next one is emitted immediately again.
        assert!(a.offer(key(1), ev.clone(), t0 + Duration::from_secs(62)).0.is_some());
    }

    #[test]
    fn aggregation_new_window_flushes_previous() {
        let mut a = Aggregator::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let ev = NewEvent::new(kind::IO_ERROR, Severity::Error, "e");
        a.offer(key(1), ev.clone(), t0);
        a.offer(key(1), ev.clone(), t0 + Duration::from_secs(10));
        a.offer(key(1), ev.clone(), t0 + Duration::from_secs(20));
        // Next occurrence after the window: summary of the old window + new first record.
        let (first, flushed) = a.offer(key(1), ev.clone(), t0 + Duration::from_secs(70));
        assert!(first.is_some());
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].count, 2);
    }

    #[test]
    fn io_error_storm_writes_few_records() {
        let d = tempfile::tempdir().unwrap();
        let log = EventLog::open(d.path(), DEFAULT_CAP_BYTES);
        let t0 = Instant::now();
        for i in 0..10_000u64 {
            let ev = NewEvent::new(kind::IO_ERROR, Severity::Error, "I/O write error");
            log.io_error_at(key((i % 2) as usize), ev, t0 + Duration::from_millis(i));
        }
        log.tick_at(t0 + Duration::from_secs(120), false);
        let page = log.query(&EventQuery::default());
        assert_eq!(page.events.len(), 4);
        let total: u64 = page.events.iter().map(|e| e.count).sum();
        assert_eq!(total, 10_000);
        assert_eq!(log.counters().io_errors, 10_000);
    }

    #[test]
    fn filters_and_migration() {
        let d = tempfile::tempdir().unwrap();
        let repair = r#"{"time":"2026-09-25T19:40:01Z","torrent_id":205,"info_hash":"aa","file_id":3,"path":"/x/a.cbz","bytes_total":100,"bytes_unreadable":4096,"bytes_zeroed":4096,"ranges_zeroed":[[0,4096]],"pieces_affected":[1,2],"method":"punch_hole"}
{"time":"2026-09-25T19:40:02Z","torrent_id":205,"info_hash":"aa","file_id":4,"path":"/x/b.cbz","bytes_total":100,"bytes_unreadable":8192,"bytes_zeroed":8192,"ranges_zeroed":[[0,8192]],"pieces_affected":[3],"method":"punch_hole"}
{"time":"2026-09-25T19:41:00Z","torrent_id":207,"info_hash":"bb","file_id":0,"path":"/y/c.cbz","bytes_total":100,"bytes_unreadable":4096,"bytes_zeroed":4096,"ranges_zeroed":[[0,4096]],"pieces_affected":[9],"method":"punch_hole"}
"#;
        let adopt = r#"{"ts":"2026-09-20T10:00:00Z","info_hash":"cc","name":"T","output_folder":"/o","from":"/o/a.part","to":"/o/a","evidence":"size_match"}
"#;
        std::fs::write(d.path().join("repair-log.jsonl"), repair).unwrap();
        std::fs::write(d.path().join("adopt-log.jsonl"), adopt).unwrap();
        let log = EventLog::open(d.path(), DEFAULT_CAP_BYTES);
        assert!(!d.path().join("repair-log.jsonl").exists());
        assert!(d.path().join("repair-log.jsonl.migrated").exists());
        let c = log.counters();
        assert_eq!(c.manual_repairs, 2);
        assert_eq!(c.auto_repairs, 0);
        assert_eq!(c.files_repaired, 3);
        assert_eq!(c.bytes_zeroed, 16384);
        assert_eq!(c.pieces_requeued, 4);
        assert_eq!(c.per_torrent.get("aa"), Some(&1));
        assert_eq!(c.since, "2026-09-20T10:00:00Z");
        assert_eq!(log.repair_count("bb"), 1);
        // Adoption is oldest -> seq 1.
        let all = log.query(&EventQuery::default());
        assert_eq!(all.events.len(), 4);
        assert_eq!(all.events.last().unwrap().kind, kind::ADOPTION);
        let q = |q: EventQuery| log.query(&q).events.len();
        assert_eq!(q(EventQuery { info_hash: Some("AA".into()), ..Default::default() }), 2);
        assert_eq!(q(EventQuery { torrent_id: Some(207), ..Default::default() }), 1);
        assert_eq!(q(EventQuery { kind: Some("adoption".into()), ..Default::default() }), 1);
        assert_eq!(q(EventQuery { severity: Some("warning".into()), ..Default::default() }), 0);
        assert_eq!(q(EventQuery { since: Some("2026-09-25T19:40:02Z".into()), ..Default::default() }), 2);
        // Re-open: no double import, counters persisted.
        drop(log);
        let log = EventLog::open(d.path(), DEFAULT_CAP_BYTES);
        assert_eq!(log.query(&EventQuery::default()).events.len(), 4);
        assert_eq!(log.counters().manual_repairs, 2);
        let r = log.reset_counters();
        assert_eq!(r.manual_repairs, 0);
        assert_eq!(log.repair_count("aa"), 0);
    }
}
