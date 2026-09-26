//! Magnets added with `defer_metadata`: the add returns at once with a reserved torrent
//! id, and metadata is resolved in the background. Until then the torrent shows up in
//! the list as "Resolving metadata" (or with the resolve error). Persisted in
//! `pending-magnets.json` next to `preferences.json`, so a restart mid-resolve keeps it.

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

/// Give up after this long when the client didn't ask for a timeout (resume retries).
pub const DEFAULT_RESOLVE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingState {
    Resolving,
    Paused,
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
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    pub added_unix: i64,
    pub state: PendingState,
    #[serde(default)]
    pub error: Option<String>,
    /// When the current resolve attempt started (for "Resolving metadata · 2m").
    #[serde(default)]
    pub attempt_unix: Option<i64>,
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
            preferred_id: Some(self.id),
            magnet_resolve_timeout: Some(
                self.timeout_secs
                    .map(Duration::from_secs)
                    .unwrap_or(DEFAULT_RESOLVE_TIMEOUT),
            ),
            ..Default::default()
        }
    }

    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.info_hash.clone())
    }

    pub fn stats(&self, now_unix: i64) -> TorrentStats {
        let (state, detail, error) = match self.state {
            PendingState::Resolving => {
                let secs = self
                    .attempt_unix
                    .map(|s| (now_unix - s).max(0) as u64)
                    .unwrap_or(0);
                let label = if secs >= 60 {
                    format!("Resolving metadata · {}m", secs / 60)
                } else {
                    "Resolving metadata".to_string()
                };
                (
                    TorrentStatsState::Initializing { paused: false },
                    StatusDetail::simple(StatusKind::ResolvingMetadata, label),
                    None,
                )
            }
            PendingState::Paused => (
                TorrentStatsState::Paused,
                StatusDetail::simple(
                    StatusKind::Paused,
                    "Paused (metadata not resolved yet)".into(),
                ),
                None,
            ),
            PendingState::Failed => {
                let e = self.error.clone().unwrap_or_else(|| "unknown error".into());
                (
                    TorrentStatsState::Error,
                    StatusDetail::simple(
                        StatusKind::Error,
                        format!("Metadata failed: {e} (Resume to retry)"),
                    ),
                    Some(format!("resolving metadata failed: {e}")),
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
        Self {
            path: Some(path),
            map: Mutex::new(magnets.into_iter().map(|m| (m.id, m)).collect()),
            tasks: Default::default(),
        }
    }

    fn save(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
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
                timeout_secs: opts.magnet_resolve_timeout.map(|d| d.as_secs()),
                added_unix: now,
                state: PendingState::Resolving,
                error: None,
                attempt_unix: Some(now),
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
                self.pending
                    .update(m.id, |m| m.attempt_unix = Some(unix_now()));
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

    async fn run_pending_resolve(self: Arc<Self>, id: usize) {
        let Some(pm) = self.pending.get(id) else {
            return;
        };
        let res = self
            .add_torrent(
                AddTorrent::Url(pm.url.clone().into()),
                Some(pm.add_options()),
            )
            .await;
        self.pending.tasks.lock().remove(&id);
        // Removed or paused meanwhile: the result no longer matters.
        let still = self
            .pending
            .get(id)
            .is_some_and(|m| m.state == PendingState::Resolving);
        match res {
            Ok(AddTorrentResponse::Added(tid, h)) => {
                self.pending.remove(id);
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
            }
            Ok(AddTorrentResponse::ListOnly(_)) => {
                self.pending.remove(id);
            }
            Err(e) => {
                if !still {
                    return;
                }
                let msg = format!("{e:#}");
                warn!(id, name = %pm.display_name(), "resolving magnet metadata failed: {msg}");
                self.pending.update(id, |m| {
                    m.state = PendingState::Failed;
                    m.error = Some(msg.clone());
                });
                self.events.emit(
                    crate::event_log::NewEvent::new(
                        crate::event_log::kind::METADATA_FAILED,
                        crate::event_log::Severity::Warning,
                        format!("Couldn't resolve metadata for {}: {msg}", pm.display_name()),
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

    /// Pause a pending magnet (stops resolving). Returns false if `id` isn't pending.
    pub fn pending_pause(&self, id: usize) -> bool {
        if !self.pending.contains_id(id) {
            return false;
        }
        self.pending.abort_task(id);
        self.pending.update(id, |m| {
            m.state = PendingState::Paused;
            m.paused = true;
        });
        true
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
        });
        self.spawn_pending_resolve(id);
        true
    }

    /// Forget a pending magnet (there are no files yet). Returns false if not pending.
    pub async fn pending_forget(self: &Arc<Self>, id: usize, source: &str) -> bool {
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
        if committed && let Err(e) = self.delete(TorrentIdOrHash::Id(id), false).await {
            warn!(
                id,
                "error removing torrent committed while being cancelled: {e:#}"
            );
        }
        self.events.emit(
            crate::event_log::NewEvent::new(
                crate::event_log::kind::METADATA_FAILED,
                crate::event_log::Severity::Info,
                format!(
                    "Removed {} before its metadata was resolved ({source})",
                    pm.display_name()
                ),
            )
            .torrent(crate::event_log::TorrentRef {
                id: Some(id),
                info_hash: pm.info_hash.clone(),
                name: pm.name.clone(),
            }),
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
            timeout_secs: None,
            added_unix: 0,
            state,
            error: Some("timed out".into()),
            attempt_unix: Some(0),
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
    fn stats_labels() {
        let s = pm(1, PendingState::Resolving).stats(125);
        assert_eq!(
            s.status_detail.as_ref().unwrap().label,
            "Resolving metadata · 2m"
        );
        assert!(matches!(s.state, TorrentStatsState::Initializing { .. }));
        let s = pm(1, PendingState::Failed).stats(0);
        assert!(matches!(s.state, TorrentStatsState::Error));
        assert!(s.error.unwrap().contains("timed out"));
        let s = pm(1, PendingState::Paused).stats(0);
        assert!(matches!(s.state, TorrentStatsState::Paused));
    }

    #[test]
    fn add_options_carry_id_and_default_timeout() {
        let o = pm(7, PendingState::Resolving).add_options();
        assert_eq!(o.preferred_id, Some(7));
        assert_eq!(o.magnet_resolve_timeout, Some(DEFAULT_RESOLVE_TIMEOUT));
        assert!(is_magnet_like("magnet:?xt=urn:btih:abc"));
        assert!(is_magnet_like(&"a".repeat(40)));
        assert!(!is_magnet_like("http://x"));
    }
}
