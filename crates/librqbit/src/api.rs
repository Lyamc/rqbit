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
        origin: crate::remove_policy::RemoveOrigin,
    ) -> Result<EmptyJsonResponse> {
        if let Some(id) = self.pending_id(idx) {
            self.session.pending_forget(id, &origin).await;
            return Ok(Default::default());
        }
        self.mgr_handle(idx)?;
        self.session
            .delete_logged(idx, false, &origin)
            .await
            .context("error forgetting torrent")?;
        Ok(Default::default())
    }

    pub async fn api_torrent_action_delete(
        &self,
        idx: TorrentIdOrHash,
        origin: crate::remove_policy::RemoveOrigin,
    ) -> Result<EmptyJsonResponse> {
        if let Some(id) = self.pending_id(idx) {
            self.session.pending_forget(id, &origin).await;
            return Ok(Default::default());
        }
        self.mgr_handle(idx)?;
        self.session
            .delete_logged(idx, true, &origin)
            .await
            .context("error deleting torrent with files")?;
        Ok(Default::default())
    }

    /// Remove following a remove policy (explicit or the saved default).
    pub async fn api_torrent_remove(
        &self,
        idx: TorrentIdOrHash,
        req: crate::remove_policy::RemoveRequest,
        wait: bool,
        origin: crate::remove_policy::RemoveOrigin,
    ) -> Result<crate::remove_policy::RemoveOutcome> {
        if let Some(id) = self.pending_id(idx) {
            // Still resolving metadata: there are no files, whatever the policy.
            let pm = self.session.pending.get(id);
            self.session.pending_forget(id, &origin).await;
            return Ok(crate::remove_policy::RemoveOutcome {
                id,
                name: pm.map(|p| p.display_name()).unwrap_or_default(),
                policy: req.policy.unwrap_or(self.session.preferences.get().remove_policy),
                result: "removed_before_metadata".into(),
                ..Default::default()
            });
        }
        self.mgr_handle(idx)?;
        let out = self
            .session
            .remove_with_policy(idx, req.policy, &origin, wait)
            .await
            .with_status(StatusCode::CONFLICT)?;
        Ok(out)
    }

    pub fn api_remove_preview(&self, ids: &[TorrentId]) -> crate::remove_policy::RemovePreview {
        self.session.remove_preview(ids)
    }

    pub fn api_download_order(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<crate::download_order::DownloadOrderView> {
        let h = self.mgr_handle(idx)?;
        self.session
            .download_order_view(&h)
            .with_status(StatusCode::CONFLICT)
    }

    pub fn api_set_download_order(
        &self,
        idx: TorrentIdOrHash,
        patch: crate::download_order::DownloadOrderPatch,
    ) -> Result<crate::download_order::DownloadOrderView> {
        let h = self.mgr_handle(idx)?;
        self.session
            .set_download_order(&h, &patch)
            .with_status(StatusCode::BAD_REQUEST)
    }

    pub fn api_torrent_rules(
        &self,
        idx: TorrentIdOrHash,
    ) -> Result<crate::torrent_rules::TorrentRulesView> {
        let h = self.mgr_handle(idx)?;
        Ok(self.session.torrent_rules_view(&h))
    }

    pub fn api_set_torrent_rules(
        &self,
        idx: TorrentIdOrHash,
        req: crate::torrent_rules::SetRulesOverride,
    ) -> Result<crate::torrent_rules::TorrentRulesView> {
        let h = self.mgr_handle(idx)?;
        let o = req.override_.map(|o| crate::torrent_rules::RulesOverride {
            stalled: o.stalled,
            seeding: o.seeding,
            speed_window: o.speed_window,
        });
        self.session
            .rules
            .set_override(&h.info_hash().as_string(), o);
        Ok(self.session.torrent_rules_view(&h))
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

    /// `POST /torrents`. Magnets are queued at once (reserved id, metadata resolved in
    /// the background, see [`crate::pending_magnets`]); nothing waits on peers or DHT
    /// unless `wait_for_metadata` is set. Only malformed input is an error; a magnet's
    /// metadata not having arrived yet never is.
    pub async fn api_add_torrent(
        &self,
        add: AddTorrent<'_>,
        opts: Option<AddTorrentOptions>,
    ) -> Result<ApiAddTorrentResponse> {
        self.api_add_torrent_with_deadline(add, opts, None).await
    }

    /// Like [`Self::api_add_torrent`]; `deadline` is the HTTP request timeout. A magnet
    /// add that waits (`wait_for_metadata`, or a `list_only` preview) stops waiting just
    /// before it: an add is then queued as a resolving placeholder, a preview answers
    /// "metadata not available yet" (`resolving: true`, no id). Neither is an error.
    pub async fn api_add_torrent_with_deadline(
        &self,
        add: AddTorrent<'_>,
        opts: Option<AddTorrentOptions>,
        deadline: Option<std::time::Duration>,
    ) -> Result<ApiAddTorrentResponse> {
        use crate::pending_magnets::{is_magnet_like, is_metadata_wait};
        let mut opts = opts.unwrap_or_default();
        let magnet_url = match &add {
            AddTorrent::Url(url) if is_magnet_like(url.trim()) => Some(url.trim().to_string()),
            _ => None,
        };
        let Some(url) = magnet_url else {
            return self.api_add_torrent_blocking(add, opts).await;
        };
        let magnet = validate_magnet(&url)?;

        if !opts.list_only && !opts.wait_for_metadata {
            let job = match opts.add_job_id.take() {
                Some(id) => Some(
                    self.session
                        .add_jobs
                        .register(id)
                        .with_status(StatusCode::BAD_REQUEST)?,
                ),
                None => None,
            };
            return self.api_queue_magnet(&url, opts, job).await;
        }

        // Waiting for metadata: explicit wait_for_metadata, or a list_only preview.
        let list_only = opts.list_only;
        let placeholder = (!list_only).then(|| placeholder_opts(&opts));
        if let Some(d) = deadline {
            let cap = d
                .saturating_sub(std::time::Duration::from_secs(2))
                .max(std::time::Duration::from_secs(1));
            opts.magnet_resolve_timeout =
                Some(opts.magnet_resolve_timeout.map_or(cap, |t| t.min(cap)));
        }
        let job_id = opts.add_job_id.clone();
        let fut = self.session.add_torrent(AddTorrent::Url(url.clone().into()), Some(opts));
        let res = match deadline {
            Some(d) => match tokio::time::timeout(d, fut).await {
                Ok(r) => r,
                Err(_) => Err(crate::pending_magnets::MetadataNotReady(format!(
                    "no metadata within the {}s request timeout",
                    d.as_secs()
                ))
                .into()),
            },
            None => fut.await,
        };
        match res {
            Err(e) if is_metadata_wait(&e) => match placeholder {
                Some(popts) => {
                    tracing::info!(
                        info_hash = ?magnet.as_id20(),
                        "no metadata yet ({e:#}); queued to keep resolving in the background"
                    );
                    let job = job_id.and_then(|id| self.session.add_jobs.job(&id));
                    self.api_queue_magnet(&url, popts, job).await
                }
                None => {
                    // list_only preview: there is nothing to list without metadata.
                    let info_hash = magnet.as_id20().map(|h| h.as_string()).unwrap_or_default();
                    Ok(ApiAddTorrentResponse {
                        id: None,
                        info_hash: info_hash.clone(),
                        details: TorrentDetailsResponse {
                            id: None,
                            info_hash,
                            name: magnet.name.clone(),
                            output_folder: String::new(),
                            total_pieces: 0,
                            torznab_category: None,
                            files: Some(vec![]),
                            stats: None,
                        },
                        output_folder: String::new(),
                        seen_peers: None,
                        resolving: true,
                        already_managed: false,
                        state: AddState::ResolvingMetadata,
                    })
                }
            },
            res => self.add_response(res),
        }
    }

    /// Queue a (validated) magnet as a resolving placeholder and answer at once.
    async fn api_queue_magnet(
        &self,
        url: &str,
        opts: AddTorrentOptions,
        job: Option<Arc<crate::add_job::AddJob>>,
    ) -> Result<ApiAddTorrentResponse> {
        use crate::add_job::AddJobStage;
        use crate::pending_magnets::DeferredAdd;
        let res = self
            .session
            .add_magnet_deferred(url, opts)
            .await
            .context("error adding torrent")
            .with_status(StatusCode::BAD_REQUEST);
        let d = match res {
            Ok(d) => d,
            Err(e) => {
                if let Some(j) = &job {
                    j.finish(AddJobStage::Failed {
                        error: format!("{e}"),
                    });
                }
                return Err(e);
            }
        };
        match d {
            d @ (DeferredAdd::Resolving(_) | DeferredAdd::AlreadyResolving(_)) => {
                let (pm, already_managed) = match d {
                    DeferredAdd::Resolving(pm) => (pm, false),
                    DeferredAdd::AlreadyResolving(pm) => (pm, true),
                    DeferredAdd::AlreadyManaged(_) => unreachable!(),
                };
                if let Some(j) = &job {
                    j.finish(AddJobStage::ResolvingInBackground { torrent_id: pm.id });
                }
                let mut details = pending_details(&pm, None);
                details.files = Some(vec![]);
                Ok(ApiAddTorrentResponse {
                    id: Some(pm.id),
                    info_hash: pm.info_hash.clone(),
                    output_folder: details.output_folder.clone(),
                    details,
                    seen_peers: None,
                    resolving: true,
                    already_managed,
                    state: AddState::ResolvingMetadata,
                })
            }
            DeferredAdd::AlreadyManaged(id) => {
                if let Some(j) = &job {
                    j.finish(AddJobStage::AlreadyManaged { torrent_id: id });
                }
                let handle = self.mgr_handle(TorrentIdOrHash::Id(id))?;
                let details = self.api_torrent_details(TorrentIdOrHash::Id(id))?;
                Ok(ApiAddTorrentResponse {
                    id: Some(id),
                    info_hash: details.info_hash.clone(),
                    details,
                    seen_peers: None,
                    output_folder: handle.output_folder().to_string_lossy().into_owned(),
                    resolving: false,
                    already_managed: true,
                    state: AddState::AlreadyManaged,
                })
            }
        }
    }

    /// .torrent bytes / http(s) URLs (full metadata): the normal add.
    async fn api_add_torrent_blocking(
        &self,
        add: AddTorrent<'_>,
        opts: AddTorrentOptions,
    ) -> Result<ApiAddTorrentResponse> {
        let res = self.session.add_torrent(add, Some(opts)).await;
        self.add_response(res)
    }

    fn add_response(
        &self,
        res: anyhow::Result<AddTorrentResponse>,
    ) -> Result<ApiAddTorrentResponse> {
        let res = match res {
            Err(e) if e.downcast_ref::<crate::session::InvalidAddInput>().is_some() => {
                return Err(ApiError::invalid_input(e));
            }
            r => r,
        };
        let response = match res
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
                    info_hash: details.info_hash.clone(),
                    details,
                    seen_peers: None,
                    resolving: false,
                    already_managed: false,
                    state: AddState::AlreadyManaged,
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
                info_hash: info_hash.as_string(),
                output_folder: output_folder.to_string_lossy().into_owned(),
                seen_peers: Some(seen_peers),
                resolving: false,
                already_managed: false,
                state: AddState::ListOnly,
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
                    info_hash: details.info_hash.clone(),
                    details,
                    seen_peers: None,
                    resolving: false,
                    already_managed: false,
                    state: AddState::Added,
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
    /// Info hash (hex), same as `details.info_hash`.
    #[serde(default)]
    pub info_hash: String,
    /// What happened: `resolving_metadata` (queued, metadata still being fetched; the
    /// torrent is listed with id `id`, or for a `list_only` preview `id` is null),
    /// `added`, `already_managed` or `list_only`.
    #[serde(default)]
    pub state: AddState,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AddState {
    ResolvingMetadata,
    #[default]
    Added,
    AlreadyManaged,
    ListOnly,
}

/// The magnet checks `POST /torrents` answers 400 for: scheme, `xt=urn:btih` /
/// `urn:btmh`, info hash length / encoding. rqbit needs a BTv1 hash, so a BTv2-only
/// (btmh) magnet is refused too.
fn validate_magnet(url: &str) -> Result<librqbit_core::magnet::Magnet> {
    let bad = |e: anyhow::Error| ApiError::invalid_input(e);
    let magnet = librqbit_core::magnet::Magnet::parse(url)
        .context("provided path is not a valid magnet URL")
        .map_err(bad)?;
    if magnet.as_id20().is_none() {
        return Err(bad(anyhow::anyhow!(
            "magnet link has only a BTv2 (urn:btmh) info hash; rqbit needs a BTv1 (urn:btih) one"
        )));
    }
    Ok(magnet)
}

/// Options a resolving placeholder keeps (see `PendingMagnet`).
fn placeholder_opts(o: &AddTorrentOptions) -> AddTorrentOptions {
    AddTorrentOptions {
        overwrite: o.overwrite,
        output_folder: o.output_folder.clone(),
        sub_folder: o.sub_folder.clone(),
        only_files: o.only_files.clone(),
        only_files_regex: o.only_files_regex.clone(),
        paused: o.paused,
        initial_peers: o.initial_peers.clone(),
        trackers: o.trackers.clone(),
        torznab_category: o.torznab_category,
        adopt_foreign_incomplete: o.adopt_foreign_incomplete.clone(),
        magnet_resolve_timeout: o.magnet_resolve_timeout,
        ..Default::default()
    }
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
