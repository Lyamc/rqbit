use std::{collections::HashSet, marker::PhantomData, net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc};

use anyhow::Context;
use buffers::ByteBufOwned;
use dht::{DhtStats, Id20};
use http::StatusCode;
use librqbit_core::torrent_metainfo::{FileDetailsAttrs, ValidatedTorrentMetaV1Info};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    WithStatus, WithStatusError,
    api_error::ApiError,
    session::{
        AddTorrent, AddTorrentOptions, AddTorrentResponse, ListOnlyResponse, Session, TorrentId,
    },
    session_stats::snapshot::SessionStatsSnapshot,
    torrent_state::{
        FileStream, ManagedTorrentHandle,
        peer::stats::snapshot::{PeerStatsFilter, PeerStatsSnapshot},
    },
    type_aliases::BF,
};

#[cfg(feature = "tracing-subscriber-utils")]
use crate::tracing_subscriber_config_utils::LineBroadcast;
#[cfg(feature = "tracing-subscriber-utils")]
use futures::Stream;
#[cfg(feature = "tracing-subscriber-utils")]
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

pub use crate::torrent_state::stats::{LiveStats, TorrentStats};

pub type Result<T> = std::result::Result<T, ApiError>;

/// Library API for use in different web frameworks.
/// Contains all methods you might want to expose with (de)serializable inputs/outputs.
#[derive(Clone)]
pub struct Api {
    session: Arc<Session>,
    rust_log_reload_tx: Option<UnboundedSender<String>>,
    #[cfg(feature = "tracing-subscriber-utils")]
    line_broadcast: Option<LineBroadcast>,
    restart_tx: Option<UnboundedSender<()>>,
}

#[derive(Debug, Clone, Copy)]
pub enum TorrentIdOrHash {
    Id(TorrentId),
    Hash(Id20),
}

impl Serialize for TorrentIdOrHash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            TorrentIdOrHash::Id(id) => id.serialize(serializer),
            TorrentIdOrHash::Hash(h) => h.as_string().serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for TorrentIdOrHash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Default)]
        struct V<'de> {
            p: PhantomData<&'de ()>,
        }

        macro_rules! visit_int {
            ($v:expr) => {{
                let tid: TorrentId = $v.try_into().map_err(|e| E::custom(format!("{e:#}")))?;
                Ok(TorrentIdOrHash::from(tid))
            }};
        }

        impl<'de> serde::de::Visitor<'de> for V<'de> {
            type Value = TorrentIdOrHash;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("integer or 40 byte info hash")
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                visit_int!(v)
            }

            fn visit_i128<E>(self, v: i128) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                visit_int!(v)
            }

            fn visit_u128<E>(self, v: u128) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                visit_int!(v)
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                visit_int!(v)
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                TorrentIdOrHash::parse(v).map_err(|e| {
                    E::custom(format!(
                        "expected integer or 40 byte info hash, couldn't parse string: {e:#}"
                    ))
                })
            }
        }

        deserializer.deserialize_any(V::default())
    }
}

impl std::fmt::Display for TorrentIdOrHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TorrentIdOrHash::Id(id) => write!(f, "{id}"),
            TorrentIdOrHash::Hash(h) => write!(f, "{h:?}"),
        }
    }
}

impl From<TorrentId> for TorrentIdOrHash {
    fn from(value: TorrentId) -> Self {
        TorrentIdOrHash::Id(value)
    }
}

impl From<Id20> for TorrentIdOrHash {
    fn from(value: Id20) -> Self {
        TorrentIdOrHash::Hash(value)
    }
}

impl<'a> TryFrom<&'a str> for TorrentIdOrHash {
    type Error = anyhow::Error;

    fn try_from(value: &'a str) -> std::result::Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TorrentIdOrHash {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        if s.len() == 40 {
            let id = Id20::from_str(s)?;
            return Ok(id.into());
        }
        let id: TorrentId = s.parse()?;
        Ok(id.into())
    }
}

#[derive(Deserialize, Default)]
pub struct ApiTorrentListOpts {
    #[serde(default)]
    pub with_stats: bool,
}


#[derive(serde::Serialize)]
pub struct AdminStatusResponse {
    pub version: String,
    pub preferences_path: String,
    pub admin_path: String,
    pub effective_http_listen_addr: Option<String>,
    pub env_http_listen_addr: Option<String>,
    pub env_basic_auth_set: bool,
    pub persisted: crate::AdminConfigPublic,
    pub restart_supported: bool,
    pub notes: Vec<String>,
}

impl Api {
    pub fn new(
        session: Arc<Session>,
        rust_log_reload_tx: Option<UnboundedSender<String>>,
        #[cfg(feature = "tracing-subscriber-utils")] line_broadcast: Option<LineBroadcast>,
    ) -> Self {
        Self {
            session,
            rust_log_reload_tx,
            restart_tx: None,
            #[cfg(feature = "tracing-subscriber-utils")]
            line_broadcast,
        }
    }

    pub fn with_restart_tx(mut self, tx: UnboundedSender<()>) -> Self {
        self.restart_tx = Some(tx);
        self
    }

    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// `idx` names a magnet still resolving metadata (and not a regular torrent).
    fn pending_id(&self, idx: TorrentIdOrHash) -> Option<usize> {
        if self.session.get(idx).is_some() {
            return None;
        }
        self.session.pending.resolve_idx(idx)
    }

    pub fn mgr_handle(&self, idx: TorrentIdOrHash) -> Result<ManagedTorrentHandle> {
        self.session
            .get(idx)
            .ok_or(ApiError::torrent_not_found(idx))
    }

    pub fn api_torrent_list(&self) -> TorrentListResponse {
        self.api_torrent_list_ext(ApiTorrentListOpts { with_stats: false })
    }

    pub fn api_torrent_list_ext(&self, opts: ApiTorrentListOpts) -> TorrentListResponse {
        let items = self.session.with_torrents(|torrents| {
            torrents
                .map(|(id, mgr)| {
                    let total_pieces = mgr
                        .metadata
                        .load()
                        .as_ref()
                        .map(|m| m.info.lengths().total_pieces())
                        .unwrap_or(0);
                    let mut r = TorrentDetailsResponse {
                        id: Some(id),
                        info_hash: mgr.shared().info_hash.as_string(),
                        name: mgr.name(),
                        output_folder: mgr
                            .output_folder()
                            .to_string_lossy()
                            .into_owned(),
                        total_pieces,
                        torznab_category: mgr.torznab_category(),

                        // These will be filled in /details and /stats endpoints
                        files: None,
                        stats: None,
                    };
                    if opts.with_stats {
                        r.stats = Some(mgr.stats());
                    }
                    r
                })
                .collect::<Vec<_>>()
        });
        let mut items = items;
        // Magnets still resolving metadata (reserved ids, no files yet).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for pm in self.session.pending.list() {
            if items.iter().any(|t| t.id == Some(pm.id) || t.info_hash == pm.info_hash) {
                continue;
            }
            items.push(pending_details(&pm, opts.with_stats.then(|| pm.stats(now))));
        }
        TorrentListResponse { torrents: items }
    }

    pub fn api_torrent_details(&self, idx: TorrentIdOrHash) -> Result<TorrentDetailsResponse> {
        if self.session.get(idx).is_none()
            && let Some(pm) = self.session.pending.resolve_idx(idx).and_then(|id| self.session.pending.get(id))
        {
            let mut d = pending_details(&pm, None);
            d.files = Some(vec![]);
            return Ok(d);
        }
        let handle = self.mgr_handle(idx)?;
        let info_hash = handle.shared().info_hash;
        let only_files = handle.only_files();
        let output_folder = handle
            .output_folder()
            .to_string_lossy()
            .into_owned()
            .to_string();
        {
            let renames = handle.file_renames();
            make_torrent_details(
                Some(handle.id()),
                &info_hash,
                handle.metadata.load().as_ref().map(|r| &r.info),
                handle.name().as_deref(),
                only_files.as_deref(),
                output_folder,
                &renames,
                handle.torznab_category(),
            )
        }
    }

    pub fn api_session_stats(&self) -> SessionStatsSnapshot {
        self.session().stats_snapshot()
    }

    pub fn torrent_file_mime_type(
        &self,
        idx: TorrentIdOrHash,
        file_idx: usize,
    ) -> Result<&'static str> {
        let handle = self.mgr_handle(idx)?;
        handle.with_metadata(|r| torrent_file_mime_type(&r.info, file_idx))?
    }

    pub fn api_peer_stats(
        &self,
        idx: TorrentIdOrHash,
        filter: PeerStatsFilter,
    ) -> Result<PeerStatsSnapshot> {
        let handle = self.mgr_handle(idx)?;
        Ok(handle
            .live()
            .with_status_error(
                StatusCode::PRECONDITION_FAILED,
                crate::Error::TorrentIsNotLive,
            )?
            .per_peer_stats_snapshot(filter))
    }

    pub async fn api_torrent_action_pause(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<EmptyJsonResponse> {
        if let Some(id) = self.pending_id(idx) {
            self.session.pending_pause(id);
            return Ok(Default::default());
        }
        let handle = self.mgr_handle(idx)?;
        self.session()
            .pause(&handle)
            .await
            .with_status(StatusCode::BAD_REQUEST)?;
        Ok(Default::default())
    }

    pub async fn api_torrent_action_start(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<EmptyJsonResponse> {
        if let Some(id) = self.pending_id(idx) {
            self.session.pending_start(id);
            return Ok(Default::default());
        }
        let handle = self.mgr_handle(idx)?;
        self.session
            .unpause(&handle)
            .await
            .with_status(StatusCode::BAD_REQUEST)?;
        Ok(Default::default())
    }

    pub async fn api_torrent_action_restart(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<EmptyJsonResponse> {
        let handle = self.mgr_handle(idx)?;
        let _ = self.session().pause(&handle).await;
        self.session
            .unpause(&handle)
            .await
            .with_status(StatusCode::BAD_REQUEST)?;
        Ok(Default::default())
    }

    /// Soft-recover torrents in error: re-init with previously_errored=true so bitfield is
    /// cleared and a full recheck keeps good pieces while bad ones are redownloaded.
    pub async fn api_torrent_action_fix_errors(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<EmptyJsonResponse> {
        let handle = self.mgr_handle(idx)?;
        // Manual fix always runs now: reset automatic-recovery backoff and requeue pieces
        // that were held back (or given up on) after I/O errors.
        let requeued = handle
            .manual_recovery_reset()
            .with_status(StatusCode::INTERNAL_SERVER_ERROR)?;
        if handle.live().is_some() {
            tracing::info!(id = handle.id(), requeued, "fix errors: recovery counters reset");
            return Ok(Default::default());
        }
        self.session
            .unpause(&handle)
            .await
            .with_status(StatusCode::BAD_REQUEST)?;
        Ok(Default::default())
    }

    /// Force a full recheck (re-verify every piece; unreadable data marks files damaged).
    pub async fn api_torrent_action_recheck(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<EmptyJsonResponse> {
        let handle = self.mgr_handle(idx)?;
        self.session
            .force_recheck(&handle)
            .await
            .with_status(StatusCode::BAD_REQUEST)?;
        Ok(Default::default())
    }

    /// Move torrents in the queue (multi-select): up / down / top / bottom.
    pub fn api_queue_move(&self, req: QueueMoveRequest) -> Result<QueueOrderResponse> {
        self.session
            .queue_move(&req.ids, req.action)
            .with_status(StatusCode::BAD_REQUEST)?;
        Ok(self.api_queue_order())
    }

    pub fn api_queue_order(&self) -> QueueOrderResponse {
        QueueOrderResponse {
            order: self.session.queue_order(),
        }
    }

    /// Start repairing damaged files (unreadable ranges) of a torrent in the background.
    /// Progress/result appear in the torrent stats under `damage.repair`.
    pub fn api_torrent_action_repair_files(
        &self,
        idx: TorrentIdOrHash,
        req: crate::repair::RepairRequest,
    ) -> Result<crate::repair::RepairStartResponse> {
        let handle = self.mgr_handle(idx)?;
        self.session
            .start_repair_files(&handle, req)
            .with_status(StatusCode::BAD_REQUEST)
    }

    pub async fn api_torrent_action_rename_file(
        &self,
        idx: TorrentIdOrHash,
        file_id: usize,
        new_path: String,
    ) -> Result<EmptyJsonResponse> {
        let handle = self.mgr_handle(idx)?;
        self.session
            .rename_file(&handle, file_id, PathBuf::from(new_path))
            .await
            .map_err(|e| ApiError::from((StatusCode::BAD_REQUEST, e)))?;
        Ok(Default::default())
    }

    pub async fn api_torrent_action_relocate(
        &self,
        idx: TorrentIdOrHash,
        destination: String,
        copy: bool,
    ) -> Result<EmptyJsonResponse> {
        let handle = self.mgr_handle(idx)?;
        self.session
            .relocate_torrent(&handle, PathBuf::from(destination), copy)
            .await
            .map_err(|e| ApiError::from((StatusCode::BAD_REQUEST, e)))?;
        Ok(Default::default())
    }

    pub fn api_get_preferences(&self) -> crate::SessionPreferences {
        self.session().preferences()
    }

    pub async fn api_set_preferences(
        &self,
        prefs: crate::SessionPreferences,
    ) -> Result<EmptyJsonResponse> {
        self.session()
            .update_preferences(prefs)
            .await
            .context("error saving preferences")
            .with_status(StatusCode::BAD_REQUEST)?;
        Ok(Default::default())
    }

    pub async fn api_reload_preferences(&self) -> Result<crate::SessionPreferences> {
        self.session()
            .reload_preferences()
            .await
            .context("error reloading preferences")
            .with_status(StatusCode::BAD_REQUEST)
    }

    pub fn api_admin_status(&self, listen_addr: Option<std::net::SocketAddr>) -> AdminStatusResponse {
        let admin = self.session().admin_config();
        let env_listen = std::env::var("RQBIT_HTTP_API_LISTEN_ADDR").ok();
        let env_auth = std::env::var("RQBIT_HTTP_BASIC_AUTH_USERPASS").ok();
        AdminStatusResponse {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            preferences_path: self.session().preferences_path().display().to_string(),
            admin_path: self.session().admin_path().display().to_string(),
            effective_http_listen_addr: listen_addr.map(|a| a.to_string()),
            env_http_listen_addr: env_listen,
            env_basic_auth_set: env_auth.is_some(),
            persisted: admin.public_view(),
            restart_supported: self.restart_tx.is_some(),
            notes: vec![
                "Live: rate limits (limits.json), soft-recover, incomplete extension, organize/completion actions, and default peer limit (preferences.json).".into(),
                "Restart required (admin.json, env overrides file): listen/announce ports, DHT/LSD/trackers, TCP/uTP, SOCKS proxy, UPnP forward, timeouts, block/allow lists, fastresume, HTTP listen/auth.".into(),
                "Not in engine yet (no fake toggles): protocol encryption, seeding ratio/time limits, sequential-download default, disk preallocation toggle, PeX disable.".into(),
                "Queueing (max active downloads/uploads/torrents) is live (preferences.json, off by default); queue order is persisted in queue.json.".into(),
                "Process restart exits with code 75 so systemd Restart=on-failure can bring the service back.".into(),
            ],
        }
    }

    pub async fn api_update_admin(
        &self,
        patch: crate::AdminConfigUpdate,
    ) -> Result<crate::AdminConfigPublic> {
        let cfg = self
            .session()
            .update_admin_config(patch)
            .await
            .context("error saving admin config")
            .with_status(StatusCode::BAD_REQUEST)?;
        Ok(cfg.public_view())
    }

    pub fn api_request_restart(&self) -> Result<EmptyJsonResponse> {
        let tx = self
            .restart_tx
            .as_ref()
            .ok_or_else(|| ApiError::from((StatusCode::NOT_IMPLEMENTED, anyhow::anyhow!("process restart is not enabled in this build/runtime"))))?;
        tx.send(())
            .context("failed to signal restart")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(Default::default())
    }


    pub async fn api_torrent_action_forget(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<EmptyJsonResponse> {
        if let Some(id) = self.pending_id(idx) {
            self.session.pending_forget(id, "forget").await;
            return Ok(Default::default());
        }
        self.session
            .delete(idx, false)
            .await
            .context("error forgetting torrent")?;
        Ok(Default::default())
    }

    pub async fn api_torrent_action_delete(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<EmptyJsonResponse> {
        if let Some(id) = self.pending_id(idx) {
            self.session.pending_forget(id, "delete").await;
            return Ok(Default::default());
        }
        self.session
            .delete(idx, true)
            .await
            .context("error deleting torrent with files")?;
        Ok(Default::default())
    }

    pub async fn api_torrent_action_update_only_files(
        &self,
        idx: TorrentIdOrHash,
        only_files: &HashSet<usize>,
    ) -> Result<EmptyJsonResponse> {
        let handle = self.mgr_handle(idx)?;
        self.session
            .update_only_files(&handle, only_files)
            .await
            .context("error updating only_files")?;
        Ok(Default::default())
    }

    pub fn api_set_rust_log(&self, new_value: String) -> Result<EmptyJsonResponse> {
        let tx = self
            .rust_log_reload_tx
            .as_ref()
            .context("rust_log_reload_tx was not set")?;
        tx.send(new_value)
            .context("noone is listening to RUST_LOG changes")?;
        Ok(Default::default())
    }

    #[cfg(feature = "tracing-subscriber-utils")]
    pub fn api_log_lines_stream(
        &self,
    ) -> Result<
        impl Stream<Item = std::result::Result<bytes::Bytes, BroadcastStreamRecvError>>
        + Send
        + Sync
        + 'static,
    > {
        Ok(self
            .line_broadcast
            .as_ref()
            .map(|sender| BroadcastStream::new(sender.subscribe()))
            .context("line_rx wasn't set")?)
    }

    pub async fn api_add_torrent(
        &self,
        add: AddTorrent<'_>,
        opts: Option<AddTorrentOptions>,
    ) -> Result<ApiAddTorrentResponse> {
        if let AddTorrent::Url(url) = &add
            && let Some(o) = opts.as_ref()
            && o.defer_metadata
            && !o.list_only
            && crate::pending_magnets::is_magnet_like(url.trim())
        {
            use crate::pending_magnets::DeferredAdd;
            let url = url.trim().to_string();
            let opts = opts.unwrap();
            match self
                .session
                .add_magnet_deferred(&url, opts)
                .await
                .context("error adding torrent")
                .with_status(StatusCode::BAD_REQUEST)?
            {
                d @ (DeferredAdd::Resolving(_) | DeferredAdd::AlreadyResolving(_)) => {
                    let (pm, already_managed) = match d {
                        DeferredAdd::Resolving(pm) => (pm, false),
                        DeferredAdd::AlreadyResolving(pm) => (pm, true),
                        DeferredAdd::AlreadyManaged(_) => unreachable!(),
                    };
                    let mut details = pending_details(&pm, None);
                    details.files = Some(vec![]);
                    return Ok(ApiAddTorrentResponse {
                        id: Some(pm.id),
                        output_folder: details.output_folder.clone(),
                        details,
                        seen_peers: None,
                        resolving: true,
                        already_managed,
                    });
                }
                DeferredAdd::AlreadyManaged(id) => {
                    let handle = self.mgr_handle(TorrentIdOrHash::Id(id))?;
                    let details = self.api_torrent_details(TorrentIdOrHash::Id(id))?;
                    return Ok(ApiAddTorrentResponse {
                        id: Some(id),
                        details,
                        seen_peers: None,
                        output_folder: handle.output_folder().to_string_lossy().into_owned(),
                        resolving: false,
                        already_managed: true,
                    });
                }
            }
        }
        let response = match self
            .session
            .add_torrent(add, opts)
            .await
            .context("error adding torrent")
            .with_status(StatusCode::BAD_REQUEST)?
        {
            AddTorrentResponse::AlreadyManaged(id, handle) => {
                let details = make_torrent_details(
                    Some(id),
                    &handle.info_hash(),
                    handle.metadata.load().as_ref().map(|r| &r.info),
                    handle.name().as_deref(),
                    handle.only_files().as_deref(),
                    handle
                        .output_folder()
                        .to_string_lossy()
                        .into_owned(),
                    &handle.file_renames(),
                    handle.torznab_category(),
                )
                .context("error making torrent details")?;
                ApiAddTorrentResponse {
                    id: Some(id),
                    details,
                    seen_peers: None,
                    resolving: false,
                    already_managed: false,
                    output_folder: handle
                        .output_folder()
                        .to_string_lossy()
                        .into_owned(),
                }
            }
            AddTorrentResponse::ListOnly(ListOnlyResponse {
                info_hash,
                info,
                only_files,
                seen_peers,
                output_folder,
                ..
            }) => ApiAddTorrentResponse {
                id: None,
                output_folder: output_folder.to_string_lossy().into_owned(),
                seen_peers: Some(seen_peers),
                resolving: false,
                already_managed: false,
                details: make_torrent_details(
                    None,
                    &info_hash,
                    Some(&info),
                    None,
                    only_files.as_deref(),
                    output_folder.to_string_lossy().into_owned().to_string(),
                    &Default::default(),
                    None,
                )
                .context("error making torrent details")?,
            },
            AddTorrentResponse::Added(id, handle) => {
                let details = make_torrent_details(
                    Some(id),
                    &handle.info_hash(),
                    handle.metadata.load().as_ref().map(|r| &r.info),
                    handle.name().as_deref(),
                    handle.only_files().as_deref(),
                    handle
                        .output_folder()
                        .to_string_lossy()
                        .into_owned(),
                    &handle.file_renames(),
                    handle.torznab_category(),
                )
                .context("error making torrent details")?;
                ApiAddTorrentResponse {
                    id: Some(id),
                    details,
                    seen_peers: None,
                    resolving: false,
                    already_managed: false,
                    output_folder: handle
                        .output_folder()
                        .to_string_lossy()
                        .into_owned(),
                }
            }
        };
        Ok(response)
    }

    pub fn api_dht_stats(&self) -> Result<DhtStats> {
        self.session
            .get_dht()
            .as_ref()
            .map(|d| d.stats())
            .ok_or(ApiError::dht_disabled())
    }

    pub fn api_dht_table(&self) -> Result<impl Serialize + use<>> {
        let dht = self.session.get_dht().ok_or(ApiError::dht_disabled())?;
        Ok(dht.with_routing_tables(|v4, v6| {
            #[derive(Serialize)]
            struct Tables<T> {
                v4: T,
                v6: T,
            }
            Tables {
                v4: v4.clone(),
                v6: v6.clone(),
            }
        }))
    }

    pub fn api_stats_v0(&self, idx: TorrentIdOrHash) -> Result<LiveStats> {
        let mgr = self.mgr_handle(idx)?;
        let live = mgr.live().context("torrent not live")?;
        Ok(LiveStats::from(&*live))
    }

    pub fn api_stats_v1(&self, idx: TorrentIdOrHash) -> Result<TorrentStats> {
        if let Some(pm) = self.pending_id(idx).and_then(|id| self.session.pending.get(id)) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            return Ok(pm.stats(now));
        }
        let mgr = self.mgr_handle(idx)?;
        Ok(mgr.stats())
    }

    pub fn api_dump_haves(&self, idx: TorrentIdOrHash) -> Result<(BF, u32)> {
        let mgr = self.mgr_handle(idx)?;
        mgr.with_chunk_tracker(|chunks| {
            let bf = BF::from_bitslice(chunks.get_have_pieces().as_slice());
            let len = chunks.get_lengths().total_pieces();
            (bf, len)
        })
        .with_status_error(
            StatusCode::PRECONDITION_FAILED,
            crate::Error::TorrentIsNotLive,
        )
    }

    pub async fn api_stream(&self, idx: TorrentIdOrHash, file_id: usize) -> Result<FileStream> {
        let mgr = self.mgr_handle(idx)?;
        Ok(mgr.stream(file_id).await?)
    }
}

#[derive(Serialize)]
pub struct TorrentListResponse {
    pub torrents: Vec<TorrentDetailsResponse>,
}

#[derive(Serialize, Deserialize)]
pub struct TorrentDetailsResponseFile {
    pub name: String,
    pub components: Vec<String>,
    pub length: u64,
    pub included: bool,
    pub attributes: FileDetailsAttrs,
}

#[derive(Default, Serialize)]
pub struct EmptyJsonResponse {}

#[derive(Serialize, Deserialize)]
pub struct TorrentDetailsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<usize>,
    pub info_hash: String,
    pub name: Option<String>,
    pub output_folder: String,

    #[serde(default)]
    pub total_pieces: u32,

    /// Newznab/Torznab category id if supplied at add time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torznab_category: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<TorrentDetailsResponseFile>>,
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub stats: Option<TorrentStats>,
}

#[derive(Serialize, Deserialize)]
pub struct ApiAddTorrentResponse {
    pub id: Option<usize>,
    pub details: TorrentDetailsResponse,
    pub output_folder: String,
    pub seen_peers: Option<Vec<SocketAddr>>,
    /// Magnet accepted with `defer_metadata`: metadata is being resolved in the
    /// background (the torrent is listed as "Resolving metadata").
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resolving: bool,
    /// Deferred magnet add whose info hash was already in rqbit (or resolving).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already_managed: bool,
}

fn pending_details(
    pm: &crate::pending_magnets::PendingMagnet,
    stats: Option<TorrentStats>,
) -> TorrentDetailsResponse {
    TorrentDetailsResponse {
        id: Some(pm.id),
        info_hash: pm.info_hash.clone(),
        name: Some(pm.display_name()),
        output_folder: pm.output_folder.clone().unwrap_or_default(),
        total_pieces: 0,
        torznab_category: pm.torznab_category,
        files: None,
        stats,
    }
}

fn make_torrent_details(
    id: Option<TorrentId>,
    info_hash: &Id20,
    info: Option<&ValidatedTorrentMetaV1Info<ByteBufOwned>>,
    name: Option<&str>,
    only_files: Option<&[usize]>,
    output_folder: String,
    renames: &std::collections::HashMap<usize, PathBuf>,
    torznab_category: Option<u32>,
) -> Result<TorrentDetailsResponse> {
    let files = match info {
        Some(info) => info
            .iter_file_details()
            .enumerate()
            .map(|(idx, d)| {
                let (name, components) = if let Some(renamed) = renames.get(&idx) {
                    let name = renamed.to_string_lossy().into_owned();
                    let components = renamed
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>();
                    (name, components)
                } else {
                    (d.filename.to_string(), d.filename.to_vec())
                };
                let included = only_files.map(|o| o.contains(&idx)).unwrap_or(true);
                TorrentDetailsResponseFile {
                    name,
                    components,
                    length: d.len,
                    included,
                    attributes: d.attrs(),
                }
            })
            .collect(),
        None => Default::default(),
    };
    let total_pieces = info.map(|i| i.lengths().total_pieces()).unwrap_or(0);
    Ok(TorrentDetailsResponse {
        id,
        info_hash: info_hash.as_string(),
        name: name
            .map(|s| s.to_owned())
            .or_else(|| info.and_then(|i| i.name().map(|n| n.into_owned()))),
        files: Some(files),
        output_folder,
        total_pieces,
        torznab_category,
        stats: None,
    })
}

fn torrent_file_mime_type(
    info: &ValidatedTorrentMetaV1Info<ByteBufOwned>,
    file_idx: usize,
) -> Result<&'static str> {
    Ok(info
        .iter_file_details()
        .nth(file_idx)
        .and_then(|d| {
            d.filename
                .iter_components()
                .last()
                .and_then(|s| mime_guess::from_path(&*s).first_raw())
        })
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot determine mime type for file",
        ))?)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct QueueMoveRequest {
    pub ids: Vec<TorrentIdOrHash>,
    pub action: crate::torrent_queue::QueueMove,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct QueueOrderResponse {
    /// Torrent ids in queue order (first = position 1).
    pub order: Vec<TorrentId>,
}
