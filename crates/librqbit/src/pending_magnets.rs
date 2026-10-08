//! Magnets whose metadata hasn't arrived yet. `POST /torrents` queues every magnet here
//! by default: the add returns at once with a reserved torrent id, and the metadata is
//! resolved in the background. Until then the torrent shows up in the list as
//! "Resolving metadata". Persisted in `pending-magnets.json` next to
//! `preferences.json`, so a restart mid-resolve keeps it.
//!
//! Waiting for metadata never fails: no peers, a slow DHT lookup or a resolve window
//! that runs out only lead to another attempt after a backoff (DHT/tracker queries are
//! paused in between), shown as "Resolving metadata (no peers yet)". A placeholder is
//! only marked failed for a real error once the metadata is there (e.g. the output
//! folder can't be used), and even then it stays listed until the user removes it.
//!
//! Paused placeholders (added with `paused=true`, or paused by the user while resolving)
//! still fetch the metadata: it is tiny, and without it the files can't be listed or
//! chosen. The DHT is only queried, not announced to (announcing is what advertises
//! us as a peer for the torrent), and no piece data is downloaded: once the metadata
//! arrives the torrent is added paused and stays paused until started.

use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use librqbit_core::magnet::Magnet;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, Session,
    api::TorrentIdOrHash,
    torrent_state::{TorrentStats, TorrentStatsState},
    torrent_status::{StatusDetail, StatusKind},
};

/// Length of one resolve attempt (DHT + trackers + peers) when the client didn't pass
/// `magnet_timeout_secs`. When it runs out the placeholder backs off and tries again;
/// it never fails.
pub const DEFAULT_ATTEMPT_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Pause between attempts: base * 2^(attempt-1), capped. No DHT / tracker traffic for
/// the placeholder while it waits. `RQBIT_METADATA_RETRY_BASE_SECS` overrides the base
/// (testing).
pub const RETRY_BACKOFF_BASE: Duration = Duration::from_secs(60);
pub const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);
/// Display hint only (was the 15-min give-up timeout): after this long without
/// metadata the status reads "Resolving metadata (no peers yet)". Never fails or
/// removes anything. `RQBIT_METADATA_STALLED_SECS` overrides it (testing).
pub const STALLED_AFTER: Duration = Duration::from_secs(15 * 60);

fn env_secs(name: &str) -> Option<Duration> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn stalled_after() -> Duration {
    env_secs("RQBIT_METADATA_STALLED_SECS").unwrap_or(STALLED_AFTER)
}

/// Pause before attempt `attempts + 1` (`attempts` = attempts that ran out so far, >= 1).
pub fn retry_backoff(attempts: u32) -> Duration {
    backoff_for(
        env_secs("RQBIT_METADATA_RETRY_BASE_SECS").unwrap_or(RETRY_BACKOFF_BASE),
        attempts,
    )
}

fn backoff_for(base: Duration, attempts: u32) -> Duration {
    let base = base.max(Duration::from_secs(1));
    let exp = attempts.saturating_sub(1).min(16);
    base.saturating_mul(1u32 << exp).min(RETRY_BACKOFF_MAX.max(base))
}

/// The torrent metadata isn't available (yet): no way to find peers, no peer answered
/// within the resolve window, or the peer stream ran dry. Never a reason to fail an
/// add; callers queue / keep the placeholder resolving instead.
#[derive(Debug)]
pub struct MetadataNotReady(pub String);

impl std::fmt::Display for MetadataNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MetadataNotReady {}

/// Whether `e` only means "metadata hasn't arrived yet".
pub fn is_metadata_wait(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<MetadataNotReady>())
}

/// Stored error texts of placeholders that older builds marked failed only because
/// metadata didn't arrive in time (migrated back to resolving on load).
fn is_legacy_wait_error(msg: &str) -> bool {
    [
        "waiting for torrent metadata",
        "no known way to resolve peers",
        "input address stream exhausted",
        "timed out after",
        "deadline has elapsed",
    ]
    .iter()
    .any(|m| msg.contains(m))
}

fn fmt_dur(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingState {
    Resolving,
    /// Legacy (older builds stopped resolving on pause): loaded as `Resolving` with
    /// `paused: true`.
    Paused,
    /// A real error after the metadata arrived (never for metadata timing).
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMagnet {
    pub id: usize,
    pub info_hash: String,
    #[serde(default)]
    pub name: Option<String>,
    /// The magnet URL (or bare info hash) as added.
    pub url: String,
    #[serde(default)]
    pub output_folder: Option<String>,
    #[serde(default)]
    pub sub_folder: Option<String>,
    #[serde(default)]
    pub only_files: Option<Vec<usize>>,
    #[serde(default)]
    pub only_files_regex: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub initial_peers: Option<Vec<SocketAddr>>,
    #[serde(default)]
    pub trackers: Option<Vec<String>>,
    #[serde(default)]
    pub torznab_category: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    /// Per-attempt resolve window (`magnet_timeout_secs`); after it the placeholder
    /// backs off and retries (it never fails).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Transfer from another client: carried to the real add once metadata is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopt_foreign_incomplete: Option<String>,
    pub added_unix: i64,
    pub state: PendingState,
    #[serde(default)]
    pub error: Option<String>,
    /// When the current resolve attempt started.
    #[serde(default)]
    pub attempt_unix: Option<i64>,
    /// Resolve attempts that ran out without metadata (since added / last resume).
    #[serde(default)]
    pub attempts: u32,
    /// Backing off: the next attempt starts at this time.
    #[serde(default)]
    pub next_retry_unix: Option<i64>,
    /// Why the last attempt ran out (e.g. "no peers answered within 10m").
    #[serde(default)]
    pub last_wait: Option<String>,
}

impl PendingMagnet {
    fn add_options(&self) -> AddTorrentOptions {
        AddTorrentOptions {
            overwrite: self.overwrite,
            output_folder: self.output_folder.clone(),
            sub_folder: self.sub_folder.clone(),
            only_files: self.only_files.clone(),
            only_files_regex: self.only_files_regex.clone(),
            paused: self.paused,
            initial_peers: self.initial_peers.clone(),
            trackers: self.trackers.clone(),
            torznab_category: self.torznab_category,
            category: self.category.clone(),
            category_source: self.category_source.clone(),
            category_id: self.category_id.clone(),
            adopt_foreign_incomplete: self.adopt_foreign_incomplete.clone(),
            preferred_id: Some(self.id),
            magnet_resolve_timeout: Some(self.attempt_window()),
            wait_for_metadata: true,
            ..Default::default()
        }
    }

    fn attempt_window(&self) -> Duration {
        self.timeout_secs
            .filter(|s| *s > 0)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_ATTEMPT_WINDOW)
    }

    pub fn category(&self) -> crate::source_category::TorrentCategory {
        crate::source_category::TorrentCategory {
            name: self.category.clone(),
            source: self.category_source.clone(),
            id: self.category_id.clone(),
            torznab: self.torznab_category,
        }
    }

    pub fn set_category(&mut self, c: crate::source_category::TorrentCategory) {
        self.category = c.name;
        self.category_source = c.source;
        self.category_id = c.id;
        self.torznab_category = c.torznab;
    }

    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.info_hash.clone())
    }

    pub fn stats(&self, now_unix: i64) -> TorrentStats {
        self.stats_with(now_unix, stalled_after())
    }

    fn stats_with(&self, now_unix: i64, stalled_after: Duration) -> TorrentStats {
        let (state, detail, error) = match self.state {
            PendingState::Resolving | PendingState::Paused => {
                let paused = self.paused || self.state == PendingState::Paused;
                let total = (now_unix - self.added_unix).max(0) as u64;
                let stalled = self.attempts > 0 || total >= stalled_after.as_secs();
                let mut label = if stalled {
                    format!("Resolving metadata (no peers yet) · {}", fmt_dur(total))
                } else if total >= 60 {
                    format!("Resolving metadata · {}", fmt_dur(total))
                } else {
                    "Resolving metadata".to_string()
                };
                if paused {
                    // Still fetching the (tiny) metadata; no data until started.
                    label = format!("Paused · {}", label.replacen("Resolving", "resolving", 1));
                }
                if let Some(next) = self.next_retry_unix.filter(|n| *n > now_unix) {
                    label.push_str(&format!(
                        " · next try in {}",
                        fmt_dur((next - now_unix) as u64)
                    ));
                }
                if paused {
                    (
                        TorrentStatsState::Paused,
                        StatusDetail::simple(StatusKind::Paused, label),
                        None,
                    )
                } else {
                    (
                        TorrentStatsState::Initializing { paused: false },
                        StatusDetail::simple(StatusKind::ResolvingMetadata, label),
                        None,
                    )
                }
            }
            PendingState::Failed => {
                let e = self.error.clone().unwrap_or_else(|| "unknown error".into());
                (
                    TorrentStatsState::Error,
                    StatusDetail::simple(
                        StatusKind::Error,
                        format!("Couldn't add after metadata arrived: {e} (Resume to retry)"),
                    ),
                    Some(format!("adding failed after metadata arrived: {e}")),
                )
            }
        };
        TorrentStats {
            state,
            file_progress: vec![],
            error,
            progress_bytes: 0,
            uploaded_bytes: 0,
            total_bytes: 0,
            finished: false,
            live: None,
            damage: None,
            status_detail: Some(detail),
            queue_position: None,
            repair_count: None,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct PendingFile {
    #[serde(default)]
    magnets: Vec<PendingMagnet>,
}

#[derive(Default)]
pub struct PendingMagnets {
    path: Option<PathBuf>,
    map: Mutex<BTreeMap<usize, PendingMagnet>>,
    tasks: Mutex<HashMap<usize, tokio::task::AbortHandle>>,
    /// Serializes `save` (snapshot + tmp write + rename): concurrent resolvers would
    /// otherwise race on the tmp file (ENOENT on rename, or an older snapshot winning).
    save_lock: Mutex<()>,
}

impl PendingMagnets {
    pub fn load(path: PathBuf) -> Self {
        let magnets = std::fs::read(&path)
            .ok()
            .and_then(|b| match serde_json::from_slice::<PendingFile>(&b) {
                Ok(f) => Some(f.magnets),
                Err(e) => {
                    warn!(?path, "error reading pending magnets: {e:#}");
                    None
                }
            })
            .unwrap_or_default();
        let magnets = magnets.into_iter().map(|mut m| {
            // Older builds marked a placeholder failed when its metadata didn't
            // arrive in time. Waiting never fails now: resolve it again.
            if m.state == PendingState::Failed
                && m.error.as_deref().is_none_or(is_legacy_wait_error)
            {
                info!(id = m.id, "placeholder was marked failed for a metadata timeout; resolving again");
                m.state = PendingState::Resolving;
                m.last_wait = m.error.take();
            }
            // Older builds stopped resolving on pause; paused placeholders now keep
            // fetching the metadata and stay paused.
            if m.state == PendingState::Paused {
                m.state = PendingState::Resolving;
                m.paused = true;
            }
            m
        });
        Self {
            path: Some(path),
            map: Mutex::new(magnets.map(|m| (m.id, m)).collect()),
            tasks: Default::default(),
            save_lock: Default::default(),
        }
    }

    fn save(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let _serialize = self.save_lock.lock();
        let f = PendingFile {
            magnets: self.map.lock().values().cloned().collect(),
        };
        let res = (|| -> anyhow::Result<()> {
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&f)?)?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        })();
        if let Err(e) = res {
            warn!(?path, "error saving pending magnets: {e:#}");
        }
    }

    pub fn get(&self, id: usize) -> Option<PendingMagnet> {
        self.map.lock().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<PendingMagnet> {
        self.map.lock().values().cloned().collect()
    }

    pub fn ids(&self) -> Vec<usize> {
        self.map.lock().keys().copied().collect()
    }

    pub fn contains_id(&self, id: usize) -> bool {
        self.map.lock().contains_key(&id)
    }

    pub fn max_id(&self) -> Option<usize> {
        self.map.lock().keys().next_back().copied()
    }

    pub fn find_hash(&self, hash: &str) -> Option<usize> {
        self.map
            .lock()
            .values()
            .find(|m| m.info_hash == hash)
            .map(|m| m.id)
    }

    pub fn resolve_idx(&self, idx: TorrentIdOrHash) -> Option<usize> {
        match idx {
            TorrentIdOrHash::Id(id) => self.contains_id(id).then_some(id),
            TorrentIdOrHash::Hash(h) => self.find_hash(&h.as_string()),
        }
    }

    fn update(&self, id: usize, f: impl FnOnce(&mut PendingMagnet)) -> bool {
        let found = self.map.lock().get_mut(&id).map(f).is_some();
        if found {
            self.save();
        }
        found
    }

    fn remove(&self, id: usize) -> Option<PendingMagnet> {
        let r = self.map.lock().remove(&id);
        if r.is_some() {
            self.save();
        }
        r
    }

    fn abort_task(&self, id: usize) {
        if let Some(h) = self.tasks.lock().remove(&id) {
            h.abort();
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Result of a deferred magnet add.
pub enum DeferredAdd {
    /// Newly queued for background resolution.
    Resolving(PendingMagnet),
    /// The same info hash is already resolving.
    AlreadyResolving(PendingMagnet),
    /// Already a regular torrent.
    AlreadyManaged(usize),
}

pub fn is_magnet_like(s: &str) -> bool {
    s.starts_with("magnet:") || (s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

impl Session {
    /// Accept a magnet at once: duplicate check by info hash, reserve an id, and resolve
    /// the metadata in the background.
    pub async fn add_magnet_deferred(
        self: &Arc<Self>,
        url: &str,
        opts: AddTorrentOptions,
    ) -> anyhow::Result<DeferredAdd> {
        let magnet = Magnet::parse(url).context("provided path is not a valid magnet URL")?;
        let info_hash = magnet
            .as_id20()
            .context("magnet link didn't contain a BTv1 infohash")?;
        if opts.output_folder.is_some() && opts.sub_folder.is_some() {
            anyhow::bail!("you can't provide both output_folder and sub_folder");
        }
        if let Some(mode) = opts.adopt_foreign_incomplete.as_deref()
            && !matches!(mode, "auto" | "qbit" | "qbittorrent")
        {
            anyhow::bail!("adopt_foreign_incomplete: unsupported mode {mode:?} (supported: auto)");
        }
        if let Some(id) = self
            .db
            .read()
            .torrents
            .iter()
            .find_map(|(id, t)| (t.info_hash() == info_hash).then_some(*id))
        {
            return Ok(DeferredAdd::AlreadyManaged(id));
        }
        let hash = info_hash.as_string();
        if let Some(id) = self.pending.find_hash(&hash) {
            return Ok(DeferredAdd::AlreadyResolving(self.pending.get(id).unwrap()));
        }
        let base = match self.persistence.as_ref() {
            Some(p) => p.next_id().await?,
            None => 0,
        };
        let pm = {
            let db = self.db.read();
            let mut map = self.pending.map.lock();
            // Re-check under the locks (concurrent adds of the same magnet).
            if let Some(m) = map.values().find(|m| m.info_hash == hash) {
                return Ok(DeferredAdd::AlreadyResolving(m.clone()));
            }
            let id = [
                Some(base),
                db.torrents.keys().copied().max().map(|m| m + 1),
                map.keys().next_back().map(|m| m + 1),
            ]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(0);
            let now = unix_now();
            let mut only_files = opts.only_files.clone();
            if only_files.is_none() {
                only_files = magnet.get_select_only();
            }
            let pm = PendingMagnet {
                id,
                info_hash: hash.clone(),
                name: magnet.name.clone(),
                url: url.to_string(),
                output_folder: opts.output_folder.clone(),
                sub_folder: opts.sub_folder.clone(),
                only_files,
                only_files_regex: opts.only_files_regex.clone(),
                overwrite: opts.overwrite,
                paused: opts.paused,
                initial_peers: opts.initial_peers.clone(),
                trackers: opts.trackers.clone(),
                torznab_category: opts.torznab_category,
                category: opts.category.clone(),
                category_source: opts.category_source.clone(),
                category_id: opts.category_id.clone(),
                timeout_secs: opts.magnet_resolve_timeout.map(|d| d.as_secs()),
                adopt_foreign_incomplete: opts.adopt_foreign_incomplete.clone(),
                added_unix: now,
                state: PendingState::Resolving,
                error: None,
                attempt_unix: Some(now),
                attempts: 0,
                next_retry_unix: None,
                last_wait: None,
            };
            map.insert(id, pm.clone());
            pm
        };
        self.pending.save();
        info!(id = pm.id, name = %pm.display_name(), "magnet accepted, resolving metadata in the background");
        self.spawn_pending_resolve(pm.id);
        Ok(DeferredAdd::Resolving(pm))
    }

    /// Restart resolution for persisted pending magnets (session start).
    pub(crate) fn resume_pending_magnets(self: &Arc<Self>) {
        for m in self.pending.list() {
            if m.state == PendingState::Resolving {
                // A backoff that was running at shutdown continues (run_pending_resolve
                // waits for next_retry_unix); otherwise start a fresh attempt.
                self.spawn_pending_resolve(m.id);
            }
        }
    }

    fn spawn_pending_resolve(self: &Arc<Self>, id: usize) {
        let this = self.clone();
        let task = tokio::spawn(async move { this.run_pending_resolve(id).await });
        if let Some(old) = self.pending.tasks.lock().insert(id, task.abort_handle()) {
            old.abort();
        }
    }

    /// Resolve a placeholder's metadata, for as long as it takes: an attempt that runs
    /// out without metadata backs off (no DHT/tracker traffic) and tries again. Stops
    /// only when the torrent was added, or the placeholder was paused / removed (which
    /// abort this task).
    async fn run_pending_resolve(self: Arc<Self>, id: usize) {
        loop {
            let Some(pm) = self.pending.get(id) else {
                return;
            };
            if pm.state != PendingState::Resolving {
                return;
            }
            if let Some(next) = pm.next_retry_unix {
                let wait = next - unix_now();
                if wait > 0 {
                    tokio::time::sleep(Duration::from_secs(wait as u64)).await;
                    continue;
                }
            }
            let now = unix_now();
            self.pending.update(id, |m| {
                m.attempt_unix = Some(now);
                m.next_retry_unix = None;
            });
            let res = self
                .add_torrent(
                    AddTorrent::Url(pm.url.clone().into()),
                    Some(pm.add_options()),
                )
                .await;
            // Removed or paused meanwhile: the result no longer matters.
            let still = self
                .pending
                .get(id)
                .is_some_and(|m| m.state == PendingState::Resolving);
            match res {
                Err(e) if is_metadata_wait(&e) => {
                    if !still {
                        self.pending.tasks.lock().remove(&id);
                        return;
                    }
                    let attempts = pm.attempts.saturating_add(1);
                    let delay = retry_backoff(attempts);
                    let reason = format!("{e:#}");
                    debug!(
                        id,
                        name = %pm.display_name(),
                        attempts,
                        retry_in_secs = delay.as_secs(),
                        "no metadata yet, backing off: {reason}"
                    );
                    self.pending.update(id, |m| {
                        m.attempts = attempts;
                        m.next_retry_unix = Some(unix_now() + delay.as_secs() as i64);
                        m.last_wait = Some(reason);
                    });
                    tokio::time::sleep(delay).await;
                    continue;
                }
                res => {
                    self.pending.tasks.lock().remove(&id);
                    self.finish_pending_resolve(id, &pm, still, res);
                    return;
                }
            }
        }
    }

    fn finish_pending_resolve(
        self: &Arc<Self>,
        id: usize,
        pm: &PendingMagnet,
        still: bool,
        res: anyhow::Result<AddTorrentResponse>,
    ) {
        match res {
            Ok(AddTorrentResponse::Added(tid, h)) => {
                // Paused / started / category edited while this attempt ran: follow
                // the latest choice.
                let latest = self.pending.get(id);
                let want_paused = latest.as_ref().map(|m| m.paused);
                if let Some(c) = latest.map(|m| m.category())
                    && c != h.category()
                {
                    h.set_category(c);
                    let (s, h2) = (self.clone(), h.clone());
                    tokio::spawn(async move { s.persist_category(&h2).await });
                }
                self.pending.remove(id);
                if let Some(want) = want_paused
                    && want != h.is_paused()
                {
                    let (s, h2) = (self.clone(), h.clone());
                    tokio::spawn(async move {
                        let r = if want { s.pause(&h2).await } else { s.unpause(&h2).await };
                        if let Err(e) = r {
                            warn!(id = tid, "error applying pause state after metadata: {e:#}");
                        }
                    });
                }
                if !still {
                    debug!(id, "pending magnet resolved after it was removed/paused");
                }
                let name = h.name().unwrap_or_else(|| pm.display_name());
                info!(id = tid, %name, "magnet metadata resolved");
                self.events.emit(
                    crate::event_log::NewEvent::new(
                        crate::event_log::kind::METADATA_RESOLVED,
                        crate::event_log::Severity::Info,
                        format!("Metadata resolved: {name}"),
                    )
                    .torrent(h.shared().torrent_ref(Some(name))),
                );
            }
            Ok(AddTorrentResponse::AlreadyManaged(tid, _)) => {
                self.pending.remove(id);
                info!(
                    id,
                    existing = tid,
                    "magnet resolved to a torrent that is already managed"
                );
                self.emit_placeholder_dropped(
                    pm,
                    format!(
                        "Removed placeholder {}: it resolved to torrent {tid}, which is already in the list; no files touched (magnet resolver)",
                        pm.display_name()
                    ),
                    serde_json::json!({"result": "already_managed", "existing_id": tid}),
                );
            }
            Ok(AddTorrentResponse::ListOnly(_)) => {
                self.pending.remove(id);
                self.emit_placeholder_dropped(
                    pm,
                    format!(
                        "Removed placeholder {}: list-only add; no files touched (magnet resolver)",
                        pm.display_name()
                    ),
                    serde_json::json!({"result": "list_only"}),
                );
            }
            Err(e) => {
                if !still {
                    return;
                }
                // Not a metadata wait (those retry above): the metadata arrived but
                // the torrent couldn't be added (e.g. output folder, adoption refused).
                // The placeholder stays listed with the error until resumed / removed.
                let msg = format!("{e:#}");
                warn!(id, name = %pm.display_name(), "adding resolved magnet failed: {msg}");
                self.pending.update(id, |m| {
                    m.state = PendingState::Failed;
                    m.error = Some(msg.clone());
                    m.next_retry_unix = None;
                });
                self.events.emit(
                    crate::event_log::NewEvent::new(
                        crate::event_log::kind::TORRENT_ERROR,
                        crate::event_log::Severity::Warning,
                        format!(
                            "Couldn't add {} after its metadata arrived: {msg}",
                            pm.display_name()
                        ),
                    )
                    .torrent(crate::event_log::TorrentRef {
                        id: Some(id),
                        info_hash: pm.info_hash.clone(),
                        name: pm.name.clone(),
                    }),
                );
            }
        }
    }

    /// Pause a pending magnet. It keeps fetching the metadata (tiny; needed to list
    /// the files) and is then added paused. Returns false if `id` isn't pending.
    pub fn pending_pause(&self, id: usize) -> bool {
        self.pending.update(id, |m| m.paused = true)
    }

    /// Set a pending magnet's category (carried to the torrent once the metadata
    /// arrives). Returns false if `id` isn't pending.
    pub fn pending_set_category(
        &self,
        id: usize,
        c: crate::source_category::TorrentCategory,
    ) -> bool {
        self.pending.update(id, |m| m.set_category(c))
    }

    /// Clear the paused flag without restarting the resolve (Add dialog finished):
    /// the torrent starts as soon as its metadata arrives.
    pub fn pending_unpause_quiet(&self, id: usize) -> bool {
        self.pending.update(id, |m| m.paused = false)
    }

    /// Resume / retry a pending magnet. Returns false if `id` isn't pending.
    pub fn pending_start(self: &Arc<Self>, id: usize) -> bool {
        if !self.pending.contains_id(id) {
            return false;
        }
        self.pending.update(id, |m| {
            m.state = PendingState::Resolving;
            m.error = None;
            m.paused = false;
            m.attempt_unix = Some(unix_now());
            m.attempts = 0;
            m.next_retry_unix = None;
        });
        self.spawn_pending_resolve(id);
        true
    }

    fn emit_placeholder_dropped(&self, pm: &PendingMagnet, msg: String, extra: serde_json::Value) {
        let mut details = crate::remove_policy::RemoveOrigin::automation("magnet resolver")
            .merge_into(serde_json::json!({
                "id": pm.id,
                "name": pm.display_name(),
                "files": "none",
            }));
        if let (Some(d), Some(e)) = (details.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                d.insert(k.clone(), v.clone());
            }
        }
        self.events.emit(
            crate::event_log::NewEvent::new(
                crate::event_log::kind::TORRENT_REMOVED,
                crate::event_log::Severity::Info,
                msg,
            )
            .torrent(crate::event_log::TorrentRef {
                id: Some(pm.id),
                info_hash: pm.info_hash.clone(),
                name: pm.name.clone(),
            })
            .details(details),
        );
    }

    /// Forget a pending magnet (there are no files yet). Returns false if not pending.
    pub async fn pending_forget(
        self: &Arc<Self>,
        id: usize,
        origin: &crate::remove_policy::RemoveOrigin,
    ) -> bool {
        let Some(pm) = self.pending.remove(id) else {
            return false;
        };
        self.pending.abort_task(id);
        // The add may have committed just before the abort: undo that too.
        let committed = self
            .db
            .read()
            .torrents
            .get(&id)
            .is_some_and(|t| t.info_hash().as_string() == pm.info_hash);
        if committed && let Err(e) = self.delete_quiet(TorrentIdOrHash::Id(id), false).await {
            warn!(
                id,
                "error removing torrent committed while being cancelled: {e:#}"
            );
        }
        self.events.emit(
            crate::event_log::NewEvent::new(
                crate::event_log::kind::TORRENT_REMOVED,
                crate::event_log::Severity::Info,
                format!(
                    "Removed {} before its metadata was resolved, no files yet ({})",
                    pm.display_name(),
                    origin.label()
                ),
            )
            .torrent(crate::event_log::TorrentRef {
                id: Some(id),
                info_hash: pm.info_hash.clone(),
                name: pm.name.clone(),
            })
            .details(origin.merge_into(serde_json::json!({
                "id": id,
                "name": pm.display_name(),
                "files": "none",
                "result": "removed_before_metadata",
                "committed": committed,
            }))),
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm(id: usize, state: PendingState) -> PendingMagnet {
        PendingMagnet {
            id,
            info_hash: format!("{id:040x}"),
            name: Some(format!("t{id}")),
            url: format!("magnet:?xt=urn:btih:{id:040x}"),
            output_folder: None,
            sub_folder: None,
            only_files: None,
            only_files_regex: None,
            overwrite: false,
            paused: false,
            initial_peers: None,
            trackers: None,
            torznab_category: None,
            category: None,
            category_source: None,
            category_id: None,
            timeout_secs: None,
            adopt_foreign_incomplete: None,
            added_unix: 0,
            state,
            error: Some("disk full".into()),
            attempt_unix: Some(0),
            attempts: 0,
            next_retry_unix: None,
            last_wait: None,
        }
    }

    #[test]
    fn persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pending-magnets.json");
        let s = PendingMagnets::load(p.clone());
        s.map.lock().insert(3, pm(3, PendingState::Resolving));
        s.save();
        let s2 = PendingMagnets::load(p);
        assert_eq!(s2.ids(), vec![3]);
        assert_eq!(s2.find_hash(&format!("{:040x}", 3)), Some(3));
        assert_eq!(s2.max_id(), Some(3));
    }

    #[test]
    fn category_persists_and_reaches_the_add() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pending-magnets.json");
        let s = PendingMagnets::load(p.clone());
        let mut m = pm(4, PendingState::Resolving);
        m.set_category(crate::source_category::TorrentCategory {
            name: None,
            source: Some("nyaa".into()),
            id: Some("1_2".into()),
            torznab: None,
        });
        s.map.lock().insert(4, m);
        s.save();
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(raw.contains("\"category_source\": \"nyaa\""), "{raw}");
        assert!(!raw.contains("\"category\":"), "unset parts aren't written: {raw}");
        let m = PendingMagnets::load(p).get(4).unwrap();
        assert_eq!(m.category().effective_torznab(), Some(5070));
        let o = m.add_options();
        assert_eq!(o.category_source.as_deref(), Some("nyaa"));
        assert_eq!(o.category_id.as_deref(), Some("1_2"));
        assert_eq!(o.category, None);
    }

    #[test]
    fn old_pending_file_without_category_loads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pending-magnets.json");
        let hash = "a".repeat(40);
        std::fs::write(
            &p,
            format!(
                r#"{{"magnets":[{{"id":2,"info_hash":"{hash}","url":"magnet:?xt=urn:btih:{hash}","torznab_category":2000,"added_unix":0,"state":"resolving"}}]}}"#
            ),
        )
        .unwrap();
        let m = PendingMagnets::load(p).get(2).unwrap();
        let c = m.category();
        assert_eq!((c.name, c.source, c.id, c.torznab), (None, None, None, Some(2000)));
    }

    #[test]
    fn stats_labels() {
        let s = pm(1, PendingState::Resolving).stats_with(30, STALLED_AFTER);
        assert_eq!(s.status_detail.as_ref().unwrap().label, "Resolving metadata");
        let s = pm(1, PendingState::Resolving).stats_with(125, STALLED_AFTER);
        assert_eq!(
            s.status_detail.as_ref().unwrap().label,
            "Resolving metadata · 2m"
        );
        assert!(matches!(s.state, TorrentStatsState::Initializing { .. }));
        assert!(s.error.is_none());
        // Past the stalled hint: still resolving, never an error.
        let s = pm(1, PendingState::Resolving).stats_with(20 * 60, STALLED_AFTER);
        assert_eq!(
            s.status_detail.as_ref().unwrap().label,
            "Resolving metadata (no peers yet) · 20m"
        );
        assert!(matches!(s.state, TorrentStatsState::Initializing { .. }));
        assert!(s.error.is_none());
        let mut m = pm(1, PendingState::Resolving);
        m.attempts = 2;
        m.next_retry_unix = Some(400);
        let s = m.stats_with(280, STALLED_AFTER);
        assert_eq!(
            s.status_detail.as_ref().unwrap().label,
            "Resolving metadata (no peers yet) · 4m · next try in 2m"
        );
        assert!(s.error.is_none());
        // Failed is only for real errors after metadata arrived.
        let s = pm(1, PendingState::Failed).stats_with(0, STALLED_AFTER);
        assert!(matches!(s.state, TorrentStatsState::Error));
        assert!(s.error.unwrap().contains("disk full"));
        let s = pm(1, PendingState::Paused).stats_with(0, STALLED_AFTER);
        assert!(matches!(s.state, TorrentStatsState::Paused));
        // Added paused: still resolving the metadata, shown as paused.
        let mut m = pm(1, PendingState::Resolving);
        m.paused = true;
        let s = m.stats_with(90, STALLED_AFTER);
        assert!(matches!(s.state, TorrentStatsState::Paused));
        assert_eq!(
            s.status_detail.as_ref().unwrap().label,
            "Paused · resolving metadata · 1m"
        );
        assert!(s.error.is_none());
    }

    #[test]
    fn add_options_carry_id_window_and_adopt() {
        let mut m = pm(7, PendingState::Resolving);
        let o = m.add_options();
        assert_eq!(o.preferred_id, Some(7));
        assert_eq!(o.magnet_resolve_timeout, Some(DEFAULT_ATTEMPT_WINDOW));
        m.timeout_secs = Some(30);
        m.adopt_foreign_incomplete = Some("auto".into());
        let o = m.add_options();
        assert_eq!(o.magnet_resolve_timeout, Some(Duration::from_secs(30)));
        assert_eq!(o.adopt_foreign_incomplete.as_deref(), Some("auto"));
        assert!(is_magnet_like("magnet:?xt=urn:btih:abc"));
        assert!(is_magnet_like(&"a".repeat(40)));
        assert!(!is_magnet_like("http://x"));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let b = RETRY_BACKOFF_BASE;
        assert_eq!(backoff_for(b, 1), b);
        assert_eq!(backoff_for(b, 2), b * 2);
        assert_eq!(backoff_for(b, 3), b * 4);
        assert_eq!(backoff_for(b, 50), RETRY_BACKOFF_MAX);
        assert_eq!(backoff_for(Duration::ZERO, 1), Duration::from_secs(1));
    }

    #[test]
    fn metadata_wait_errors_are_recognised() {
        let e = anyhow::Error::new(MetadataNotReady("no peers".into())).context("error adding");
        assert!(is_metadata_wait(&e));
        assert!(!is_metadata_wait(&anyhow::anyhow!("disk full")));
    }

    #[test]
    fn legacy_timeout_failures_resolve_again_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pending-magnets.json");
        let s = PendingMagnets::load(p.clone());
        let mut timed_out = pm(1, PendingState::Failed);
        timed_out.error = Some(
            "timed out after 900s waiting for torrent metadata from peers (magnet may be dead or poorly seeded)".into(),
        );
        s.map.lock().insert(1, timed_out);
        s.map.lock().insert(2, pm(2, PendingState::Failed));
        s.save();
        let s2 = PendingMagnets::load(p);
        let m1 = s2.get(1).unwrap();
        assert_eq!(m1.state, PendingState::Resolving);
        assert!(m1.error.is_none());
        assert_eq!(s2.get(2).unwrap().state, PendingState::Failed);
    }

    #[test]
    fn legacy_paused_placeholders_keep_resolving_paused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pending-magnets.json");
        let s = PendingMagnets::load(p.clone());
        s.map.lock().insert(3, pm(3, PendingState::Paused));
        s.save();
        let m = PendingMagnets::load(p).get(3).unwrap();
        assert_eq!(m.state, PendingState::Resolving);
        assert!(m.paused);
    }
}
