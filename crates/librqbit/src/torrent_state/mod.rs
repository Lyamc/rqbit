pub mod initializing;
pub mod live;
pub mod paused;
pub mod stats;
mod streaming;
pub mod utils;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::bail;
use arc_swap::ArcSwapOption;
use buffers::ByteBufOwned;
use bytes::Bytes;
use futures::FutureExt;
use futures::future::BoxFuture;
use librqbit_core::hash_id::Id20;
use librqbit_core::lengths::Lengths;

use librqbit_core::spawn_utils::spawn_with_cancel;
use librqbit_core::torrent_metainfo::ValidatedTorrentMetaV1Info;
pub use live::*;
use parking_lot::RwLock;

use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::debug_span;
use tracing::trace;
use tracing::warn;

use crate::Session;
use crate::chunk_tracker::ChunkTracker;
use crate::file_info::FileInfo;
use crate::limits::LimitsConfig;
use crate::session::TorrentId;
use crate::spawn_utils::BlockingSpawner;
use crate::storage::BoxStorageFactory;
use crate::stream_connect::StreamConnector;
use crate::torrent_state::stats::LiveStats;
use crate::type_aliases::FileInfos;
use crate::type_aliases::PeerStream;

use initializing::TorrentStateInitializing;

use self::paused::TorrentStatePaused;
pub use self::stats::{TorrentStats, TorrentStatsState};
pub use self::streaming::FileStream;

// State machine transitions.
//
// - error -> initializing
// - initializing -> paused
// - paused -> live
// - live -> paused
//
// - initializing -> error
// - live -> error
pub enum ManagedTorrentState {
    Initializing(Arc<TorrentStateInitializing>),
    Paused(TorrentStatePaused),
    Live(Arc<TorrentStateLive>),
    Error(anyhow::Error),

    // This is used when swapping between states, outside world should never see it.
    None,
}

impl ManagedTorrentState {
    pub fn name(&self) -> &'static str {
        match self {
            ManagedTorrentState::Initializing(_) => "initializing",
            ManagedTorrentState::Paused(_) => "paused",
            ManagedTorrentState::Live(_) => "live",
            ManagedTorrentState::Error(_) => "error",
            ManagedTorrentState::None => "<invalid: none>",
        }
    }

    fn assert_paused(self) -> TorrentStatePaused {
        match self {
            Self::Paused(paused) => paused,
            _ => panic!("Expected paused state"),
        }
    }

    pub(crate) fn take(&mut self) -> Self {
        std::mem::replace(self, Self::None)
    }
}

pub(crate) struct ManagedTorrentLocked {
    // The torrent might not be in "paused" state technically,
    // but the intention might be for it to stay paused.
    //
    // This should change only on "unpause".
    pub(crate) paused: bool,
    pub(crate) state: ManagedTorrentState,
    pub(crate) only_files: Option<Vec<usize>>,
}

#[derive(Default)]
pub(crate) struct ManagedTorrentOptions {
    pub force_tracker_interval: Option<Duration>,
    pub peer_connect_timeout: Option<Duration>,
    pub peer_read_write_timeout: Option<Duration>,
    pub peer_max_request_window: Option<usize>,
    pub allow_overwrite: bool,
    #[allow(dead_code)] // initial folder; runtime uses current_output_folder
    pub output_folder: PathBuf,
    pub ratelimits: LimitsConfig,
    pub initial_peers: Vec<SocketAddr>,
    pub peer_limit: Option<usize>,
    #[cfg(feature = "disable-upload")]
    pub _disable_upload: bool,
}

impl ManagedTorrentOptions {
    #[cfg(feature = "disable-upload")]
    pub fn disable_upload(&self) -> bool {
        self._disable_upload
    }

    #[cfg(not(feature = "disable-upload"))]
    pub const fn disable_upload(&self) -> bool {
        false
    }
}

// Torrent bencodee "info" + some precomputed fields based on it for frequent access.
pub struct TorrentMetadata {
    pub info: ValidatedTorrentMetaV1Info<ByteBufOwned>,
    pub torrent_bytes: Bytes,
    pub info_bytes: Bytes,
    pub file_infos: FileInfos,
}

impl TorrentMetadata {
    pub(crate) fn new(
        info: ValidatedTorrentMetaV1Info<ByteBufOwned>,
        torrent_bytes: Bytes,
        info_bytes: Bytes,
    ) -> anyhow::Result<Self> {
        let file_infos = info
            .iter_file_details_ext()
            .map(|fd| {
                Ok::<_, anyhow::Error>(FileInfo {
                    relative_filename: fd.details.filename.to_pathbuf(),
                    offset_in_torrent: fd.offset,
                    piece_range: fd.pieces,
                    len: fd.details.len,
                    attrs: fd.details.attrs(),
                })
            })
            .collect::<anyhow::Result<Vec<FileInfo>>>()?;

        Ok(Self {
            info,
            torrent_bytes,
            info_bytes,
            file_infos,
        })
    }

    pub fn lengths(&self) -> &Lengths {
        self.info.lengths()
    }
}

/// Common information about torrent shared among all possible states.
///
// The reason it's not inlined into ManagedTorrent is to break the Arc cycle:
// ManagedTorrent contains the current torrent state, which in turn needs access to a bunch
// of stuff, but it shouldn't access the state.
pub struct ManagedTorrentShared {
    pub id: TorrentId,
    pub info_hash: Id20,
    pub(crate) spawner: BlockingSpawner,
    pub trackers: HashSet<url::Url>,
    pub peer_id: Id20,
    pub span: tracing::Span,
    pub(crate) options: ManagedTorrentOptions,
    pub(crate) connector: Arc<StreamConnector>,
    pub(crate) storage_factory: BoxStorageFactory,
    pub(crate) session: Weak<Session>,

    /// Effective on-disk output folder (may change after move-completed / relocate).
    pub(crate) current_output_folder: RwLock<PathBuf>,
    /// Per-file relative path overrides (file_id -> new relative path). A file moved
    /// on its own ("Move files individually as they complete") has an absolute path here
    /// until the whole torrent has moved.
    pub(crate) file_renames: RwLock<HashMap<usize, PathBuf>>,
    /// Torrent folder that files are being moved into one by one (individual mode).
    pub(crate) move_dest: RwLock<Option<PathBuf>>,
    /// Serializes moves of this torrent's files.
    pub(crate) move_lock: parking_lot::Mutex<()>,

    /// Category (name, source, source id, Torznab number); set at add, editable.
    pub(crate) category: RwLock<crate::source_category::TorrentCategory>,

    // "dn" from magnet link
    pub(crate) magnet_name: Option<String>,

    pub(crate) client_name_and_version: String,

    /// Files that hit unrecoverable I/O errors, and the state of any repair.
    pub(crate) damage: crate::repair::DamageTracker,
    /// Queue hold flag, active move/rename, transfer activity (for status_detail).
    pub(crate) runtime: crate::torrent_status::RuntimeFlags,
}

impl ManagedTorrentShared {
    pub(crate) fn torrent_ref(&self, name: Option<String>) -> crate::event_log::TorrentRef {
        crate::event_log::TorrentRef {
            id: Some(self.id),
            info_hash: self.info_hash.as_string(),
            name,
        }
    }

    /// Append to the session event log (no-op when the session is gone).
    pub(crate) fn emit_event(&self, ev: crate::event_log::NewEvent) {
        if let Some(s) = self.session.upgrade() {
            s.events.emit(ev);
        }
    }

    pub(crate) fn event_log(&self) -> Option<Arc<crate::event_log::EventLog>> {
        self.session.upgrade().map(|s| s.events.clone())
    }

    /// A read error while checking files (fastresume validation / full check). On EIO the
    /// file is marked damaged, so automatic repair (when enabled) can fix it once the
    /// torrent is running. A missing or short file is just "not downloaded yet": ignored.
    pub(crate) fn note_check_read_error(
        &self,
        file_id: usize,
        piece: u32,
        e: &anyhow::Error,
        name: Option<String>,
        relative: Option<PathBuf>,
    ) {
        use crate::event_log::{NewEvent, Severity, kind};
        if crate::repair::anyhow_is_missing_data(e) {
            return;
        }
        let eio = crate::repair::anyhow_is_eio(e);
        let msg = format!("{e:#}");
        let (newly, _) = self.damage.record_failure(Some(file_id), piece, eio, &msg);
        if !newly {
            return;
        }
        let path = self
            .file_rename(file_id)
            .or(relative)
            .map(|p| self.output_folder().join(p).to_string_lossy().into_owned());
        warn!(
            id = self.id,
            info_hash = ?self.info_hash,
            file_id,
            piece,
            "file marked as damaged: unreadable data found while checking files: {msg}"
        );
        self.emit_event(
            NewEvent::new(
                kind::DAMAGE_DETECTED,
                Severity::Warning,
                format!(
                    "Unreadable data found while checking files (file {file_id}); marked damaged"
                ),
            )
            .torrent(self.torrent_ref(name))
            .file(Some(file_id), path)
            .details(serde_json::json!({
                "source": "check",
                "piece": piece,
                "eio": eio,
                "error": msg,
            })),
        );
    }

    pub(crate) fn client_name_and_version(&self) -> &str {
        &self.client_name_and_version
    }

    pub fn output_folder(&self) -> PathBuf {
        self.current_output_folder.read().clone()
    }

    pub fn set_output_folder(&self, path: PathBuf) {
        *self.current_output_folder.write() = path;
    }

    pub fn file_rename(&self, file_id: usize) -> Option<PathBuf> {
        self.file_renames.read().get(&file_id).cloned()
    }

    pub fn set_file_rename(&self, file_id: usize, path: PathBuf) {
        self.file_renames.write().insert(file_id, path);
    }

    pub fn set_file_renames_map(&self, map: HashMap<usize, PathBuf>) {
        *self.file_renames.write() = map;
    }
}

/// How a torrent's files sit in its output folder.
#[derive(Debug, Clone)]
pub struct TorrentLayout {
    pub single_file: bool,
    /// Multi-file torrent whose output folder is its own `<TorrentName>` folder.
    pub own_folder: bool,
    /// Folder name for the torrent (its name, or the info hash if unusable).
    pub folder_name: String,
}

/// What a relocation did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RelocateReport {
    pub from: PathBuf,
    pub output_folder: PathBuf,
    pub method: crate::relocate::MoveMethod,
    pub whole_folder: bool,
    pub moved_files: usize,
    pub copy: bool,
}

pub struct ManagedTorrent {
    // Static torrent configuration that doesn't change.
    pub shared: Arc<ManagedTorrentShared>,
    // Torrent metadata. Maybe be None when the magnet is resolving (not implemented yet)
    pub metadata: ArcSwapOption<TorrentMetadata>,
    pub(crate) state_change_notify: Notify,
    pub(crate) locked: RwLock<ManagedTorrentLocked>,
}

impl ManagedTorrent {
    pub fn id(&self) -> TorrentId {
        self.shared.id
    }

    pub fn name(&self) -> Option<String> {
        if let Some(m) = &*self.metadata.load() {
            return m
                .info
                .name()
                .map(|n| n.into_owned())
                .or_else(|| self.shared.magnet_name.clone());
        }
        self.shared.magnet_name.clone()
    }

    pub fn shared(&self) -> &ManagedTorrentShared {
        &self.shared
    }

    /// The resolved on-disk folder this torrent's files are written under.
    pub fn output_folder(&self) -> PathBuf {
        self.shared.output_folder()
    }

    pub fn file_renames(&self) -> HashMap<usize, PathBuf> {
        self.shared.file_renames.read().clone()
    }

    /// The Torznab number given at add or edited (not the one derived from the table).
    pub fn torznab_category(&self) -> Option<u32> {
        self.shared.category.read().torznab
    }

    pub fn category(&self) -> crate::source_category::TorrentCategory {
        self.shared.category.read().clone()
    }

    pub fn set_category(&self, c: crate::source_category::TorrentCategory) {
        *self.shared.category.write() = c;
    }

    /// Rename a single file (or its relative path including folders) while the torrent
    /// is active. Piece mapping stays the same; only the on-disk path changes.
    pub fn rename_file(&self, file_id: usize, new_relative_path: PathBuf) -> anyhow::Result<()> {
        if new_relative_path.as_os_str().is_empty() {
            bail!("new path must not be empty");
        }
        if new_relative_path.is_absolute() {
            bail!("new path must be relative");
        }
        let metadata = self.metadata.load();
        let metadata = metadata.as_ref().context("torrent is not resolved")?;
        if file_id >= metadata.file_infos.len() {
            bail!("file_id out of range");
        }
        if metadata.file_infos[file_id].attrs.padding {
            bail!("cannot rename padding file");
        }

        let _op = self
            .shared
            .runtime
            .begin_op(crate::torrent_status::ActiveOp::Renaming);
        let rename = |files: &crate::type_aliases::FileStorage| -> anyhow::Result<()> {
            files.rename_file(&self.shared, metadata, file_id, &new_relative_path)?;
            self.shared.set_file_rename(file_id, new_relative_path.clone());
            Ok(())
        };

        let g = self.locked.read();
        match &g.state {
            ManagedTorrentState::Live(live) => rename(&live.files)?,
            ManagedTorrentState::Paused(paused) => rename(&paused.files)?,
            ManagedTorrentState::Initializing(_) => {
                bail!("cannot rename while initializing")
            }
            ManagedTorrentState::Error(_) => bail!("cannot rename torrent in error state"),
            ManagedTorrentState::None => bail!("bug: torrent is in empty state"),
        }
        drop(g);
        Ok(())
    }

    /// How the torrent's files sit in its output folder.
    pub fn layout(&self) -> anyhow::Result<TorrentLayout> {
        let metadata = self.metadata.load();
        let metadata = metadata.as_ref().context("torrent is not resolved")?;
        Ok(self.layout_with(metadata))
    }

    fn layout_with(&self, metadata: &TorrentMetadata) -> TorrentLayout {
        let multi = metadata.file_infos.len() >= 2;
        let name = metadata
            .info
            .name()
            .map(|n| n.into_owned())
            .or_else(|| self.shared.magnet_name.clone())
            .unwrap_or_default();
        // A usable folder name: one normal path component.
        let leaf_ok = !name.is_empty()
            && Path::new(&name).components().count() == 1
            && matches!(
                Path::new(&name).components().next(),
                Some(std::path::Component::Normal(_))
            );
        let folder_name = if leaf_ok {
            name.clone()
        } else {
            self.shared.info_hash.as_string()
        };
        let out = self.shared.output_folder();
        let session_root = self
            .shared
            .session
            .upgrade()
            .map(|s| s.get_default_output_folder().to_path_buf());
        let own_folder = multi
            && out.file_name().map(|f| f.to_string_lossy() == folder_name.as_str()) == Some(true)
            && session_root
                .as_ref()
                .is_none_or(|r| r != &out && !r.starts_with(&out));
        TorrentLayout {
            single_file: !multi,
            own_folder,
            folder_name,
        }
    }

    /// The torrent's folder when moved into `parent` ("move to the completed folder"):
    /// `<parent>/<TorrentName>` for a multi-file torrent (its current folder name when it
    /// has its own folder), `parent` itself for a single file. A `parent` that already ends
    /// in that name is used as is.
    pub fn into_destination(&self, parent: &Path) -> anyhow::Result<PathBuf> {
        let l = self.layout()?;
        if l.single_file {
            return Ok(parent.to_path_buf());
        }
        let leaf = if l.own_folder {
            self.shared
                .output_folder()
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(&l.folder_name))
        } else {
            PathBuf::from(&l.folder_name)
        };
        if parent.file_name() == Some(leaf.as_os_str()) {
            return Ok(parent.to_path_buf());
        }
        Ok(parent.join(leaf))
    }

    /// Folder others' torrents must not be inside for a whole-folder move.
    fn folder_is_exclusive(&self, folder: &Path) -> bool {
        let Some(session) = self.shared.session.upgrade() else {
            return true;
        };
        let others = session.with_torrents(|ts| {
            ts.filter(|(id, _)| *id != self.shared.id)
                .map(|(_, t)| t.shared.output_folder())
                .collect::<Vec<_>>()
        });
        !others.iter().any(|o| o.starts_with(folder))
    }

    /// Where each file is now and where it belongs inside the torrent folder.
    fn file_locations(&self, metadata: &TorrentMetadata) -> Vec<crate::relocate::FileLoc> {
        let out = self.shared.output_folder();
        let move_dest = self.shared.move_dest.read().clone();
        metadata
            .file_infos
            .iter()
            .enumerate()
            .filter(|(_, fi)| !fi.attrs.padding)
            .map(|(file_id, fi)| {
                let cur = self
                    .shared
                    .file_rename(file_id)
                    .unwrap_or_else(|| fi.relative_filename.clone());
                let rel = if cur.is_absolute() {
                    move_dest
                        .as_ref()
                        .and_then(|d| cur.strip_prefix(d).ok().map(|p| p.to_path_buf()))
                        .unwrap_or_else(|| fi.relative_filename.clone())
                } else {
                    cur.clone()
                };
                crate::relocate::FileLoc {
                    file_id,
                    current: out.join(&cur),
                    rel,
                }
            })
            .collect()
    }

    /// Record where the files are after a move: output folder plus per-file paths
    /// (relative where possible).
    fn apply_locations(
        &self,
        metadata: &TorrentMetadata,
        output_folder: &Path,
        paths: &[(usize, PathBuf)],
    ) {
        self.shared.set_output_folder(output_folder.to_path_buf());
        let mut all_inside = true;
        {
            let mut renames = self.shared.file_renames.write();
            for (file_id, p) in paths {
                match p.strip_prefix(output_folder) {
                    Ok(r) if r == metadata.file_infos[*file_id].relative_filename => {
                        renames.remove(file_id);
                    }
                    Ok(r) => {
                        renames.insert(*file_id, r.to_path_buf());
                    }
                    Err(_) => {
                        all_inside = false;
                        renames.insert(*file_id, p.clone());
                    }
                }
            }
        }
        if all_inside {
            *self.shared.move_dest.write() = None;
        }
    }

    /// Move or copy the torrent to `new_output_folder` (the torrent's folder itself, see
    /// [`Self::into_destination`]) and keep seeding from there; no recheck. See
    /// [`crate::relocate`] for the rules (whole folder at once when it is the torrent's own
    /// folder, never overwriting, verified copies across file systems).
    pub fn relocate_output(
        &self,
        new_output_folder: PathBuf,
        copy: bool,
    ) -> anyhow::Result<RelocateReport> {
        let metadata = self.metadata.load();
        let metadata = metadata.as_ref().context("torrent is not resolved")?;
        let _serial = self.shared.move_lock.lock();

        let _op = self
            .shared
            .runtime
            .begin_op(crate::torrent_status::ActiveOp::Moving);
        let old = self.shared.output_folder();
        let layout = self.layout_with(metadata);
        let files = self.file_locations(metadata);
        let whole_folder = layout.own_folder && old.is_dir() && self.folder_is_exclusive(&old);
        let plan = crate::relocate::Plan {
            old_output: &old,
            new_output: &new_output_folder,
            files: &files,
            whole_folder,
            own_folder: layout.own_folder,
            single_file: layout.single_file,
            copy,
        };

        let g = self.locked.read();
        let storage = match &g.state {
            ManagedTorrentState::Live(live) => &live.files,
            ManagedTorrentState::Paused(paused) => &paused.files,
            ManagedTorrentState::Initializing(_) => {
                bail!("cannot relocate while initializing")
            }
            ManagedTorrentState::Error(_) => bail!("cannot relocate torrent in error state"),
            ManagedTorrentState::None => bail!("bug: torrent is in empty state"),
        };
        let mut outcome = storage.relocate_files(&self.shared, metadata, &mut || {
            crate::relocate::execute(&plan)
        })?;
        drop(g);
        self.apply_locations(metadata, &outcome.output_folder, &outcome.paths);
        let report = RelocateReport {
            from: old,
            output_folder: outcome.output_folder.clone(),
            method: outcome.method,
            whole_folder: outcome.whole_folder,
            moved_files: outcome.moved_files,
            copy,
        };
        if let Some(e) = outcome.error.take() {
            return Err(e.context(format!(
                "moving to {:?} stopped ({} file(s) moved; the torrent knows where each file is)",
                new_output_folder, report.moved_files
            )));
        }
        Ok(report)
    }

    /// "Move files individually as they complete": move one finished file into
    /// `dest_root` (the torrent's folder at the destination) and keep seeding it from
    /// there while the rest keeps downloading. The incomplete extension is dropped on the
    /// way. Never overwrites. `Ok(None)` when there was nothing to do.
    pub fn move_finished_file(
        &self,
        file_id: usize,
        dest_root: &Path,
        incomplete_ext: Option<&str>,
    ) -> anyhow::Result<Option<PathBuf>> {
        let metadata = self.metadata.load();
        let metadata = metadata.as_ref().context("torrent is not resolved")?;
        let fi = metadata.file_infos.get(file_id).context("no such file")?;
        if fi.attrs.padding {
            return Ok(None);
        }
        let _serial = self.shared.move_lock.lock();
        let out = self.shared.output_folder();
        let cur_rel = self
            .shared
            .file_rename(file_id)
            .unwrap_or_else(|| fi.relative_filename.clone());
        let current = out.join(&cur_rel);
        if current.starts_with(dest_root) || cur_rel.is_absolute() {
            return Ok(None);
        }
        let mut rel = cur_rel.clone();
        if let Some(ext) = incomplete_ext.filter(|e| !e.is_empty()) {
            let s = rel.to_string_lossy().into_owned();
            if let Some(stripped) = s.strip_suffix(ext).filter(|t| !t.is_empty()) {
                rel = PathBuf::from(stripped);
            }
        }
        let dst = dest_root.join(&rel);
        if dst.symlink_metadata().is_ok() {
            bail!("{dst:?} already exists; not overwriting (the file stays where it is)");
        }
        let g = self.locked.read();
        let storage = match &g.state {
            ManagedTorrentState::Live(live) => &live.files,
            ManagedTorrentState::Paused(paused) => &paused.files,
            _ => return Ok(None),
        };
        let new_path = storage.move_one_file(&self.shared, metadata, file_id, &mut |src| {
            crate::relocate::move_path(src, &dst, false)?;
            if let Some(p) = src.parent() {
                crate::relocate::remove_empty_dirs(p, &out, false);
            }
            Ok(dst.clone())
        })?;
        drop(g);
        self.shared.set_file_rename(file_id, new_path.clone());
        *self.shared.move_dest.write() = Some(dest_root.to_path_buf());
        Ok(Some(new_path))
    }

    pub fn with_metadata<R>(
        &self,
        mut f: impl FnMut(&Arc<TorrentMetadata>) -> R,
    ) -> anyhow::Result<R> {
        let r = self.metadata.load();
        let r = r.as_ref().context("torrent is not resolved")?;
        Ok(f(r))
    }

    pub fn info_hash(&self) -> Id20 {
        self.shared.info_hash
    }

    pub fn only_files(&self) -> Option<Vec<usize>> {
        self.locked.read().only_files.clone()
    }

    pub fn with_state<R>(&self, f: impl FnOnce(&ManagedTorrentState) -> R) -> R {
        f(&self.locked.read().state)
    }

    pub(crate) fn with_state_mut<R>(&self, f: impl FnOnce(&mut ManagedTorrentState) -> R) -> R {
        f(&mut self.locked.write().state)
    }

    pub(crate) fn with_chunk_tracker<R>(
        &self,
        f: impl FnOnce(&ChunkTracker) -> R,
    ) -> anyhow::Result<R> {
        let g = self.locked.read();
        match &g.state {
            ManagedTorrentState::Paused(p) => Ok(f(&p.chunk_tracker)),
            ManagedTorrentState::Live(l) => Ok(f(l
                .lock_read("chunk_tracker")
                .get_chunks()
                .context("error getting chunks")?)),
            _ => bail!("no chunk tracker, torrent neither paused nor live"),
        }
    }

    /// Get the live state if the torrent is live.
    pub fn live(&self) -> Option<Arc<TorrentStateLive>> {
        let g = self.locked.read();
        match &g.state {
            ManagedTorrentState::Live(live) => Some(live.clone()),
            _ => None,
        }
    }

    // Get live torrent but wait a bit until it's initialized if it is
    pub(crate) async fn live_wait_initializing(
        &self,
        duration: Duration,
    ) -> Option<Arc<TorrentStateLive>> {
        timeout(duration, self.wait_until_initialized())
            .await
            .ok()?
            .ok()?;
        self.live()
    }

    fn stop_with_error(&self, error: anyhow::Error) {
        let mut g = self.locked.write();

        match g.state.take() {
            ManagedTorrentState::Live(live) => {
                if let Err(err) = live.pause() {
                    warn!(
                        id = self.shared.id,
                        info_hash = ?self.shared.info_hash,
                        "error pausing live torrent during fatal error handling: {err:#}",
                    );
                }
            }
            ManagedTorrentState::Error(e) => {
                warn!(
                    id = self.shared.id,
                    info_hash = ?self.shared.info_hash,
                    "bug: torrent already was in error state when trying to stop it. Previous error was: {e:#}",
                );
            }
            ManagedTorrentState::None => {
                warn!(
                    id = self.shared.id,
                    info_hash = ?self.shared.info_hash,
                    "bug: torrent encountered in None state during fatal error handling"
                )
            }
            _ => {}
        };

        self.state_change_notify.notify_waiters();

        self.shared.emit_event(
            crate::event_log::NewEvent::new(
                crate::event_log::kind::TORRENT_ERROR,
                crate::event_log::Severity::Error,
                format!("Torrent stopped with an error: {error:#}"),
            )
            .torrent(self.shared.torrent_ref(self.name())),
        );

        g.state = ManagedTorrentState::Error(error)
    }

    /// peer_rx: the peer stream. If start_paused=false, must be set.
    /// start_paused: if set, the torrent will initialize (check file integrity), but will not start
    pub(crate) fn start(
        self: &Arc<Self>,
        peer_rx: Option<PeerStream>,
        start_paused: bool,
    ) -> anyhow::Result<()> {
        fn _start<'a>(
            t: &'a Arc<ManagedTorrent>,
            peer_rx: Option<PeerStream>,
            start_paused: bool,
            session: Arc<Session>,
            g: Option<parking_lot::RwLockWriteGuard<'a, ManagedTorrentLocked>>,
            token: CancellationToken,
        ) -> anyhow::Result<()> {
            let mut g = g.unwrap_or_else(|| t.locked.write());

            match &g.state {
                ManagedTorrentState::Live(_) => {
                    bail!("torrent is already live");
                }
                ManagedTorrentState::Initializing(init) => {
                    let init = init.clone();
                    init.clear_pause_request();
                    if !init.try_start_check() {
                        return Ok(());
                    }

                    let t = t.clone();
                    let span = t.shared().span.clone();
                    let token = token.clone();

                    spawn_with_cancel(
                        debug_span!(parent: span.clone(), "initialize_and_start"),
                        "initialize_and_start",
                        token.clone(),
                        async move {
                            let concurrent_init_semaphore =
                                session.concurrent_initialize_semaphore.clone();
                            let _permit = concurrent_init_semaphore
                                .acquire()
                                .await
                                .context("bug: concurrent init semaphore was closed")?;
                            init.mark_checking();

                            let check_result = init.check().await;
                            init.finish_check();

                            match check_result {
                                Ok(paused) => {
                                    let mut g = t.locked.write();
                                    if let ManagedTorrentState::Initializing(_) = &g.state {
                                    } else {
                                        debug!(
                                            "no need to start torrent anymore, as it switched state from initializing"
                                        );
                                        return Ok(());
                                    }

                                    g.state = ManagedTorrentState::Paused(paused);
                                    t.state_change_notify.notify_waiters();
                                    _start(&t, peer_rx, start_paused, session, Some(g), token)
                                }
                                Err(err) => {
                                    if init.is_pause_requested() {
                                        debug!("initial check paused");
                                        t.state_change_notify.notify_waiters();
                                        return Ok(());
                                    }

                                    let result = anyhow::anyhow!("{:?}", err);
                                    t.locked.write().state = ManagedTorrentState::Error(err);
                                    t.state_change_notify.notify_waiters();
                                    Err(result)
                                }
                            }
                        },
                    );
                    Ok(())
                }
                ManagedTorrentState::Paused(_) => {
                    if start_paused {
                        return Ok(());
                    }
                    // Queueing: over the active limits -> hold (internally paused, not
                    // user-paused). The queue manager starts it when a slot frees up.
                    if session.queue_should_hold(t.id()) {
                        g.paused = true;
                        t.shared.runtime.set_queue_held(true);
                        t.state_change_notify.notify_waiters();
                        session.queue.kick.notify_one();
                        debug!(id = t.id(), "queued (over active torrent limits)");
                        return Ok(());
                    }
                    t.shared.runtime.set_queue_held(false);
                    t.shared.runtime.reset_activity();
                    let paused = g.state.take().assert_paused();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let live = TorrentStateLive::new(paused, tx, token.clone())?;
                    g.state = ManagedTorrentState::Live(live.clone());
                    t.state_change_notify.notify_waiters();

                    spawn_fatal_errors_receiver(t, rx, token);
                    if let Some(peer_rx) = peer_rx {
                        spawn_peer_adder(&live, peer_rx);
                    }
                    Ok(())
                }
                ManagedTorrentState::Error(_) => {
                    let metadata = t.metadata.load_full().expect("TODO");
                    let initializing = Arc::new(TorrentStateInitializing::new(
                        t.shared.clone(),
                        metadata.clone(),
                        g.only_files.clone(),
                        t.shared
                            .storage_factory
                            .create_and_init(t.shared(), &metadata)?,
                        true,
                    ));
                    g.state = ManagedTorrentState::Initializing(initializing.clone());
                    t.state_change_notify.notify_waiters();

                    // Recurse.
                    _start(t, peer_rx, start_paused, session, Some(g), token)
                }
                ManagedTorrentState::None => bail!("bug: torrent is in empty state"),
            }
        }

        let session = self
            .shared
            .session
            .upgrade()
            .context("session is dead, cannot start torrent")?;
        let mut g = self.locked.write();
        g.paused = start_paused;
        let cancellation_token = session.cancellation_token().child_token();

        _start(
            self,
            peer_rx,
            start_paused,
            session,
            Some(g),
            cancellation_token,
        )
    }

    /// Stop the torrent so that the next start re-verifies every piece from scratch
    /// (fastresume bitfield cleared). Unreadable data found by that check marks files
    /// damaged.
    pub(crate) fn prepare_recheck(&self) -> anyhow::Result<()> {
        if self.shared.damage.is_repair_running() {
            bail!("a repair is running; retry when it finishes");
        }
        let mut g = self.locked.write();
        match &g.state {
            ManagedTorrentState::Initializing(_) => bail!("torrent is already checking its files"),
            ManagedTorrentState::None => bail!("bug: torrent is in empty state"),
            ManagedTorrentState::Live(live) => {
                // Dropping the paused state closes the files.
                let _paused = live.pause()?;
            }
            ManagedTorrentState::Paused(_) | ManagedTorrentState::Error(_) => {}
        }
        // The Error state restarts through a full check with the bitfield cleared.
        g.state = ManagedTorrentState::Error(anyhow::anyhow!("full recheck requested"));
        self.state_change_notify.notify_waiters();
        Ok(())
    }

    /// User-paused (persisted). Torrents held by queue limits are not user-paused.
    pub fn is_paused(&self) -> bool {
        self.locked.read().paused && !self.shared.runtime.queue_held()
    }

    /// Held by queue limits.
    pub fn is_queue_held(&self) -> bool {
        self.shared.runtime.queue_held()
    }

    /// Pause the torrent if it's live.
    pub(crate) fn pause(&self) -> anyhow::Result<()> {
        let mut g = self.locked.write();
        match &g.state {
            ManagedTorrentState::Live(live) => {
                let paused = live.pause()?;
                g.state = ManagedTorrentState::Paused(paused);
                g.paused = true;
                self.state_change_notify.notify_waiters();
                Ok(())
            }
            ManagedTorrentState::Initializing(init) => {
                let init = init.clone();
                g.paused = true;
                init.request_pause();
                self.state_change_notify.notify_waiters();
                Ok(())
            }
            ManagedTorrentState::Paused(_) => {
                bail!("torrent is already paused");
            }
            ManagedTorrentState::Error(_) => {
                bail!("can't pause torrent in error state")
            }
            ManagedTorrentState::None => bail!("bug: torrent is in empty state"),
        }
    }

    /// Get stats.
    pub fn stats(&self) -> TorrentStats {
        use stats::TorrentStatsState as S;
        let mut resp = TorrentStats {
            total_bytes: self
                .metadata
                .load()
                .as_ref()
                .map(|r| r.info.lengths().total_length())
                .unwrap_or_default(),
            file_progress: Vec::new(),
            state: S::Error,
            error: None,
            progress_bytes: 0,
            uploaded_bytes: 0,
            finished: false,
            live: None,
            damage: None,
            status_detail: None,
            queue_position: None,
            repair_count: None,
        };
        use crate::torrent_status as ts;
        let mut engine = ts::EngineState::Error;
        let mut check_progress = 0f64;
        let mut live_view: Option<ts::LiveView> = None;
        let user_paused;

        {
            let g = self.locked.read();
            user_paused = g.paused && !self.shared.runtime.queue_held();
            match &g.state {
                ManagedTorrentState::Initializing(i) => {
                    resp.state = S::Initializing { paused: g.paused };
                    resp.progress_bytes = i.checked_bytes.load(Ordering::Relaxed);
                    engine = ts::EngineState::Initializing {
                        checking: i.is_checking(),
                        check_requested: i.is_check_requested(),
                        user_paused: g.paused,
                    };
                    if resp.total_bytes > 0 {
                        check_progress = resp.progress_bytes as f64 / resp.total_bytes as f64;
                    }
                }
                ManagedTorrentState::Paused(p) => {
                    resp.state = S::Paused;
                    engine = ts::EngineState::Paused;
                    let hns = p.hns();
                    resp.total_bytes = hns.total();
                    resp.progress_bytes = hns.progress();
                    resp.finished = hns.finished();
                    resp.file_progress = p.chunk_tracker.per_file_have_bytes().to_owned();
                }
                ManagedTorrentState::Live(l) => {
                    resp.state = S::Live;
                    engine = ts::EngineState::Live;
                    let live_stats = LiveStats::from(l.as_ref());
                    live_view = Some(ts::LiveView {
                        download_bps: live_stats.download_speed.as_bytes(),
                        upload_bps: live_stats.upload_speed.as_bytes(),
                        peers_live: live_stats.snapshot.peer_stats.live,
                        secs_since_data: self.shared.runtime.observe_fetched(
                            live_stats.snapshot.fetched_bytes,
                            std::time::Instant::now(),
                        ),
                    });
                    let hns = l.get_hns().unwrap_or_default();
                    resp.total_bytes = hns.total();
                    resp.progress_bytes = hns.progress();
                    resp.finished = hns.finished();
                    resp.uploaded_bytes = l.get_uploaded_bytes();
                    resp.file_progress = l
                        .lock_read("file_progress")
                        .get_chunks()
                        .ok()
                        .map(|c| c.per_file_have_bytes().to_owned())
                        .unwrap_or_default();
                    resp.live = Some(live_stats);
                }
                ManagedTorrentState::Error(e) => {
                    resp.state = S::Error;
                    resp.error = Some(format!("{e:?}"))
                }
                ManagedTorrentState::None => {
                    resp.state = S::Error;
                    resp.error = Some("bug: torrent in broken \"None\" state".to_string());
                }
            }
        }

        let metadata = self.metadata.load();
        resp.damage = self.shared.damage.snapshot(|file_id| {
            self.shared
                .file_rename(file_id)
                .or_else(|| {
                    metadata
                        .as_ref()
                        .and_then(|m| m.file_infos.get(file_id))
                        .map(|fi| fi.relative_filename.clone())
                })
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("file {file_id}"))
        });

        let session = self.shared.session.upgrade();
        resp.queue_position = session
            .as_ref()
            .and_then(|s| s.queue.position(&self.shared.info_hash));
        resp.repair_count = session
            .as_ref()
            .map(|s| s.events.repair_count(&self.shared.info_hash.as_string()))
            .filter(|c| *c > 0);
        let _ = user_paused;
        let damage = resp.damage.as_ref();
        let repair = damage
            .and_then(|d| d.repair.as_ref())
            .filter(|r| r.state == crate::repair::RepairState::Running)
            .map(|r| ts::RepairView {
                waiting_for_slot: r.waiting_for_slot,
                progress: if r.total_bytes > 0 {
                    r.scanned_bytes as f64 / r.total_bytes as f64
                } else {
                    0.0
                },
            });
        let retry = damage.and_then(|d| d.recovery.as_ref()).map(|r| ts::RetryView {
            pieces_waiting: r.pieces_waiting,
            next_retry_in_secs: r.next_retry_in_secs,
        });
        let attention = self.shared.runtime.attention();
        resp.status_detail = Some(ts::derive_status(&ts::StatusInputs {
            engine,
            metadata_resolved: metadata.is_some(),
            finished: resp.finished,
            check_progress,
            queue_held: self.shared.runtime.queue_held(),
            queue_position: resp.queue_position,
            active_op: self.shared.runtime.active_op(),
            repair,
            needs_attention: damage.map(|d| d.needs_attention).unwrap_or(false),
            attention_note: attention.as_deref(),
            queue_eta_secs: session
                .as_ref()
                .and_then(|s| s.queue.seed_eta_secs(self.shared.id)),
            retry,
            live: live_view,
            error: resp.error.as_deref(),
            disk_full: damage
                .and_then(|d| d.disk_full.as_ref())
                .map(|d| d.message.as_str()),
        }));

        resp
    }

    #[inline(never)]
    pub fn wait_until_initialized(&self) -> BoxFuture<'_, anyhow::Result<()>> {
        async move {
            // TODO: rewrite, this polling is horrible
            loop {
                let done = self.with_state(|s| match s {
                    ManagedTorrentState::Initializing(_) => Ok(false),
                    ManagedTorrentState::Error(e) => bail!("{:?}", e),
                    ManagedTorrentState::None => bail!("bug: torrent state is None"),
                    _ => Ok(true),
                })?;
                if done {
                    return Ok(());
                }
                let _ = timeout(
                    Duration::from_millis(100),
                    self.state_change_notify.notified(),
                )
                .await;
            }
        }
        .boxed()
    }

    #[inline(never)]
    pub fn wait_until_completed(&self) -> BoxFuture<'_, anyhow::Result<()>> {
        async move {
            // TODO: rewrite, this polling is horrible
            let live = loop {
                let live = self.with_state(|s| match s {
                    ManagedTorrentState::Initializing(_) | ManagedTorrentState::Paused(_) => {
                        Ok(None)
                    }
                    ManagedTorrentState::Live(l) => Ok(Some(l.clone())),
                    ManagedTorrentState::Error(e) => bail!("{:?}", e),
                    ManagedTorrentState::None => bail!("bug: torrent state is None"),
                })?;
                if let Some(live) = live {
                    break live;
                }
                let _ = timeout(Duration::from_secs(1), self.state_change_notify.notified()).await;
            };

            live.wait_until_completed().await;
            Ok(())
        }
        .boxed()
    }

    // Returns true if needed to unpause torrent.
    // This is just implementation detail - it's easier to pause/unpause than to tinker with internals.
    pub(crate) fn update_only_files(&self, only_files: &HashSet<usize>) -> anyhow::Result<()> {
        let metadata = self.metadata.load();
        let metadata = metadata.as_ref().context("torrent is not resolved")?;
        let file_count = metadata.file_infos.len();
        for f in only_files.iter().copied() {
            if f >= file_count {
                anyhow::bail!("only_files contains invalid value {f}")
            }
        }

        // if live, need to update chunk tracker
        // - if already finished: need to pause, then unpause (to reopen files etc)
        // if paused, need to update chunk tracker

        let mut g = self.locked.write();
        match &mut g.state {
            ManagedTorrentState::Initializing(_) => bail!("can't update initializing torrent"),
            ManagedTorrentState::Error(_) => {}
            ManagedTorrentState::None => {}
            ManagedTorrentState::Paused(p) => {
                p.update_only_files(only_files)?;
            }
            ManagedTorrentState::Live(l) => {
                l.update_only_files(only_files)?;
            }
        };

        g.only_files = Some(only_files.iter().copied().collect());
        Ok(())
    }
}

pub type ManagedTorrentHandle = Arc<ManagedTorrent>;

fn spawn_fatal_errors_receiver(
    state: &Arc<ManagedTorrent>,
    rx: tokio::sync::oneshot::Receiver<anyhow::Error>,
    token: CancellationToken,
) {
    let span = state.shared.span.clone();
    let id = state.shared.id;
    let info_hash = state.shared.info_hash;
    let state = Arc::downgrade(state);
    spawn_with_cancel::<&'static str>(
        debug_span!(parent: span, "fatal_errors_receiver"),
        "fatal_errors_receiver",
        token,
        async move {
            let e = match rx.await {
                Ok(e) => e,
                Err(_) => return Ok(()),
            };
            if let Some(state) = state.upgrade() {
                state.stop_with_error(e);
            } else {
                warn!(
                    ?id,
                    ?info_hash,
                    "tried to stop the torrent with error, but couldn't upgrade the arc"
                );
            }
            Ok(())
        },
    );
}

fn spawn_peer_adder(live: &Arc<TorrentStateLive>, mut peer_rx: PeerStream) {
    live.spawn(
        debug_span!(parent: live.torrent().span.clone(), "external_peer_adder"),
        format!("[{}]external_peer_adder", live.shared.id),
        {
            let live = live.clone();
            async move {
                let live = {
                    let weak = Arc::downgrade(&live);
                    drop(live);
                    weak
                };

                loop {
                    match timeout(Duration::from_secs(5), peer_rx.next()).await {
                        Ok(Some(peer)) => {
                            trace!(?peer, "received peer");
                            let live = match live.upgrade() {
                                Some(live) => live,
                                None => return Ok(()),
                            };
                            live.add_peer_if_not_seen(peer)?;
                        }
                        Ok(None) => {
                            debug!("peer_rx closed, closing peer adder");
                            return Ok(());
                        }
                        // If timeout, check if the torrent is live.
                        Err(_) if live.strong_count() == 0 => {
                            debug!("timed out waiting for peers, torrent isn't live, closing peer adder");
                            return Ok(());
                        }
                        Err(_) => continue,
                    }
                }
            }
        },
    );
}
