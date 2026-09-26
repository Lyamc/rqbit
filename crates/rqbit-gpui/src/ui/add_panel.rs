//! "Add torrents" panel. Mirrors the web UI's Add modal (`AddModal.tsx`,
//! `StagingQueue.tsx`) as far as practical:
//!
//! - magnet links / http(s) .torrent URLs, one per line (plus anything the
//!   web UI's source detector finds in pasted text);
//! - .torrent files (native file picker / OS drag-and-drop; in the browser a
//!   file input / page drop, see `files.rs`); .txt/.magnet files are scanned
//!   for magnets;
//! - a staging list with per-item state (queued, server stage while adding,
//!   added, already in rqbit, failed + retry, cancelled);
//! - an Advanced section (output folder, overwrite, concurrent adds),
//!   collapsed by default.
//!
//! Every add carries an `add_job_id`; the server's progress is polled with
//! `GET /add_jobs/{id}` and cancels go through `POST /add_jobs/{id}/cancel`,
//! exactly like the web UI (magnets also send `magnet_timeout_secs=170` and
//! give up client-side after 3 minutes; other adds after 15 minutes).
//!
//! Not ported (web UI only for now): Browse server, Transfer from other
//! client, and server-side .zip extraction.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{Context, Entity, EventEmitter, SharedString, Window, div, prelude::*, px};

use super::files::PickedFile;
use super::text_input::TextInput;
use super::{theme, widgets};
use crate::api::{
    AddJobStatus, AddSource, AddTorrentOpts, AddTorrentResponse, ApiClient, ApiFuture,
};
use crate::sources::{self, SourceKind};
use crate::time::Instant;

const DEFAULT_CONCURRENCY: usize = 4;
/// Client-side give-up for magnets; the server is asked to stop a bit
/// earlier (`magnet_timeout_secs`) so a dead magnet fails with its error.
const MAGNET_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const MAGNET_SERVER_TIMEOUT_SECS: u64 = 170;
/// Other adds can wait a long time for a disk slot behind hash checks.
const OTHER_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const JOB_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub enum AddPanelEvent {
    Close,
    /// At least one torrent was added (refresh the list now).
    Added,
}

#[derive(Clone)]
enum ItemSource {
    Url { text: String, magnet: bool },
    File { bytes: Arc<Vec<u8>> },
}

#[derive(Clone, Debug, PartialEq)]
enum ItemStatus {
    Ready,
    Queued,
    Running,
    Added(Option<usize>),
    AlreadyInRqbit(Option<usize>),
    Failed(String),
    Cancelled,
}

impl ItemStatus {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            ItemStatus::Added(_)
                | ItemStatus::AlreadyInRqbit(_)
                | ItemStatus::Failed(_)
                | ItemStatus::Cancelled
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CancelState {
    None,
    Pending,
    Cancelled,
    AlreadyAdded,
}

struct InFlight {
    job_id: String,
    cancel: CancelState,
    timed_out: bool,
    magnet: bool,
    timeout: Duration,
}

struct StagedItem {
    key: u64,
    label: String,
    origin: SharedString,
    source: Option<ItemSource>,
    status: ItemStatus,
    started: Option<Instant>,
    stage: Option<AddJobStatus>,
    stage_since: Option<Instant>,
    note: Option<String>,
    /// Set when the torrent ended up in rqbit although the add was cancelled.
    added_id: Option<usize>,
    job: Option<InFlight>,
    /// Part of the current / last "Add" batch (progress line).
    in_batch: bool,
}

struct Work {
    key: u64,
    job_id: String,
    timeout: Duration,
    fut: ApiFuture<AddTorrentResponse>,
}

pub struct AddPanel {
    client: ApiClient,
    urls: Entity<TextInput>,
    output_folder: Entity<TextInput>,
    overwrite: bool,
    concurrency: usize,
    advanced_open: bool,
    items: Vec<StagedItem>,
    next_key: u64,
    running: bool,
    stop_requested: bool,
    workers: usize,
    batch_added: usize,
    /// Output folder captured when Add was pressed.
    batch_output_folder: Option<String>,
    message: Option<String>,
}

impl EventEmitter<AddPanelEvent> for AddPanel {}

static JOB_SEQ: AtomicU64 = AtomicU64::new(0);

/// Unique enough id for the server's add-job table (it only has to be
/// unique among this server's recent jobs).
fn new_job_id() -> String {
    let now = crate::time::SystemTime::now()
        .duration_since(crate::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seq = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    let mix = (now.subsec_nanos() as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(17)
        ^ seq.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    format!("add-gpui-{:x}-{:08x}", now.as_millis(), mix as u32)
}

fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn format_duration(d: Duration) -> String {
    let m = (d.as_secs_f64() / 60.0).round() as u64;
    if m >= 1 {
        format!("{m} min")
    } else {
        format!("{} s", d.as_secs())
    }
}

/// In-flight label from what the server reports (web UI's `stageLabel`).
fn stage_label(st: Option<&AddJobStatus>) -> String {
    let Some(st) = st else {
        return "sending to server…".into();
    };
    match st.stage.as_str() {
        "fetching_torrent" => "downloading .torrent…".into(),
        "resolving_metadata" => "resolving metadata…".into(),
        "adopting" => "checking existing files…".into(),
        "waiting_for_server" => "waiting for server (busy checking other torrents)…".into(),
        "adding" => {
            let what = match st.step.as_deref() {
                Some("saving") => "saving",
                Some("starting") => "starting",
                _ => "creating files",
            };
            let busy = if st.busy == Some(true) {
                " (server busy checking other torrents)"
            } else {
                ""
            };
            format!("adding: {what}{busy}…")
        }
        "added" | "already_managed" => "added, finishing…".into(),
        _ => "sending to server…".into(),
    }
}

fn timeout_error(magnet: bool, timeout: Duration) -> String {
    if magnet {
        format!(
            "Timed out after {} waiting for torrent metadata — no peer sent it. The magnet may be dead or poorly seeded; retry later or use a .torrent file.",
            format_duration(timeout)
        )
    } else {
        format!(
            "Gave up after {} waiting for the server; the add was cancelled (nothing added).",
            format_duration(timeout)
        )
    }
}

impl AddPanel {
    pub fn new(client: ApiClient, cx: &mut Context<Self>) -> Self {
        Self {
            client,
            urls: cx.new(|cx| {
                TextInput::multiline("magnet:?xt=urn:btih:…  or  https://…/file.torrent", 5, cx)
            }),
            output_folder: cx.new(|cx| TextInput::new("", "Leave empty for session default", cx)),
            overwrite: true,
            concurrency: DEFAULT_CONCURRENCY,
            advanced_open: false,
            items: Vec::new(),
            next_key: 1,
            running: false,
            stop_requested: false,
            workers: 0,
            batch_added: 0,
            batch_output_folder: None,
            message: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    fn push_item(
        &mut self,
        label: String,
        origin: &'static str,
        source: Option<ItemSource>,
        status: ItemStatus,
    ) {
        let key = self.next_key;
        self.next_key += 1;
        self.items.push(StagedItem {
            key,
            label,
            origin: origin.into(),
            source,
            status,
            started: None,
            stage: None,
            stage_since: None,
            note: None,
            added_id: None,
            job: None,
            in_batch: false,
        });
    }

    fn has_url(&self, text: &str) -> bool {
        self.items
            .iter()
            .any(|i| matches!(&i.source, Some(ItemSource::Url { text: t, .. }) if t == text))
    }

    fn stage_url(&mut self, text: String, magnet: bool, origin: &'static str) -> bool {
        if self.has_url(&text) {
            return false;
        }
        let label = sources::display_name_for_source(&text);
        self.push_item(
            label,
            origin,
            Some(ItemSource::Url { text, magnet }),
            ItemStatus::Ready,
        );
        true
    }

    /// Sources typed/pasted in the URL box that are not staged yet.
    fn pasted_not_queued(&self, cx: &gpui::App) -> Vec<sources::ParsedSource> {
        sources::extract_torrent_sources(self.urls.read(cx).text())
            .into_iter()
            .filter(|s| !self.has_url(&s.value))
            .collect()
    }

    fn stage_pasted(&mut self, cx: &mut Context<Self>) -> usize {
        let pasted = self.pasted_not_queued(cx);
        let mut n = 0;
        for s in pasted {
            let magnet = s.kind == SourceKind::Magnet || sources::is_magnet(&s.value);
            if self.stage_url(s.value, magnet, "url") {
                n += 1;
            }
        }
        self.urls.update(cx, |t, cx| t.set_text("", cx));
        n
    }

    /// Stage picked / dropped files (.torrent; .txt/.magnet/.md are scanned
    /// for magnets like the web UI does).
    pub fn add_files(
        &mut self,
        files: Vec<PickedFile>,
        origin: &'static str,
        cx: &mut Context<Self>,
    ) {
        let mut torrents = 0;
        let mut other = 0;
        for f in files {
            let lower = f.name.to_ascii_lowercase();
            if lower.ends_with(".torrent") {
                let dup = self.items.iter().any(|i| {
                    i.label == f.name
                        && matches!(&i.source, Some(ItemSource::File { bytes }) if bytes.len() == f.bytes.len())
                });
                if !dup {
                    self.push_item(
                        f.name,
                        origin,
                        Some(ItemSource::File {
                            bytes: Arc::new(f.bytes),
                        }),
                        ItemStatus::Ready,
                    );
                    torrents += 1;
                }
            } else if lower.ends_with(".txt")
                || lower.ends_with(".magnet")
                || lower.ends_with(".md")
            {
                let text = String::from_utf8_lossy(&f.bytes);
                for s in sources::extract_torrent_sources(&text) {
                    let magnet = s.kind == SourceKind::Magnet;
                    if self.stage_url(s.value, magnet, "file") {
                        other += 1;
                    }
                }
            } else {
                let error = if lower.ends_with(".zip") {
                    "zip files are only unpacked by the web UI's Add (server-side extract)"
                } else {
                    "not a .torrent or magnet text file"
                };
                self.push_item(f.name, origin, None, ItemStatus::Failed(error.into()));
            }
        }
        self.message = Some(match (torrents, other) {
            (0, 0) => "No new torrents in the chosen files.".into(),
            (t, 0) => format!("{t} .torrent file(s) staged."),
            (t, o) => format!("{t} .torrent file(s) and {o} link(s) from text files staged."),
        });
        cx.notify();
    }

    fn choose_files(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_family = "wasm")]
        {
            // Files arrive through the page-level channel (see RqbitWindow).
            let _ = cx;
            super::files::open_picker();
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Add torrents".into()),
            });
            cx.spawn(async move |this, cx| {
                let paths = match rx.await {
                    Ok(Ok(Some(p))) => p,
                    Ok(Ok(None)) | Err(_) => return,
                    Ok(Err(e)) => {
                        this.update(cx, |p, cx| {
                            p.message = Some(format!("File picker unavailable: {e:#}"));
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                };
                let read = cx
                    .background_executor()
                    .spawn(async move { super::files::read_paths(&paths) })
                    .await;
                this.update(cx, |p, cx| p.add_read_results(read, "file", cx))
                    .ok();
            })
            .detach();
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn add_read_results(
        &mut self,
        read: Vec<Result<PickedFile, (String, String)>>,
        origin: &'static str,
        cx: &mut Context<Self>,
    ) {
        let mut ok = Vec::new();
        for r in read {
            match r {
                Ok(f) => ok.push(f),
                Err((name, e)) => self.push_item(name, origin, None, ItemStatus::Failed(e)),
            }
        }
        self.add_files(ok, origin, cx);
    }

    fn startable(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == ItemStatus::Ready && i.source.is_some())
            .count()
    }

    fn start_import(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        self.stage_pasted(cx);
        for i in &mut self.items {
            i.in_batch = false;
            if i.status == ItemStatus::Ready && i.source.is_some() {
                i.status = ItemStatus::Queued;
                i.in_batch = true;
                i.note = None;
            }
        }
        let queued = self.items.iter().filter(|i| i.in_batch).count();
        if queued == 0 {
            cx.notify();
            return;
        }
        self.running = true;
        self.stop_requested = false;
        self.batch_added = 0;
        self.message = None;
        let folder = self.output_folder.read(cx).text().trim().to_owned();
        self.batch_output_folder = (!folder.is_empty()).then_some(folder);
        let n = self.concurrency.min(queued).max(1);
        self.workers = n;
        for _ in 0..n {
            let client = self.client.clone();
            cx.spawn(async move |this, cx| {
                loop {
                    let work = this
                        .update(cx, |p, cx| {
                            let w = p.claim_next();
                            if let Some(w) = &w {
                                p.spawn_job_watchers(w.key, w.job_id.clone(), w.timeout, cx);
                                cx.notify();
                            }
                            w
                        })
                        .ok()
                        .flatten();
                    let Some(work) = work else { break };
                    let res = cx.background_executor().spawn(work.fut).await;
                    let final_stage = if res.is_ok() {
                        cx.background_executor()
                            .spawn(client.get_add_job(&work.job_id))
                            .await
                            .ok()
                    } else {
                        None
                    };
                    if this
                        .update(cx, |p, cx| {
                            p.finish(work.key, &work.job_id, res, final_stage, cx)
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                this.update(cx, |p, cx| {
                    p.workers = p.workers.saturating_sub(1);
                    if p.workers == 0 {
                        p.running = false;
                        p.stop_requested = false;
                        if p.batch_added > 0 {
                            cx.emit(AddPanelEvent::Added);
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn claim_next(&mut self) -> Option<Work> {
        if self.stop_requested {
            return None;
        }
        let overwrite = self.overwrite;
        let item = self
            .items
            .iter_mut()
            .find(|i| i.status == ItemStatus::Queued)?;
        let source = item.source.clone()?;
        let magnet = matches!(source, ItemSource::Url { magnet: true, .. });
        let timeout = if magnet {
            MAGNET_TIMEOUT
        } else {
            OTHER_TIMEOUT
        };
        let job_id = new_job_id();
        item.status = ItemStatus::Running;
        item.started = Some(Instant::now());
        item.stage = None;
        item.stage_since = None;
        item.note = None;
        item.added_id = None;
        item.job = Some(InFlight {
            job_id: job_id.clone(),
            cancel: CancelState::None,
            timed_out: false,
            magnet,
            timeout,
        });
        let key = item.key;
        let opts = AddTorrentOpts {
            overwrite,
            output_folder: self.batch_output_folder.clone(),
            magnet_timeout_secs: magnet.then_some(MAGNET_SERVER_TIMEOUT_SECS),
            add_job_id: Some(job_id.clone()),
            // Our own timer cancels first; this only bounds a dead connection.
            timeout: Some(timeout + Duration::from_secs(60)),
        };
        let src = match source {
            ItemSource::Url { text, .. } => AddSource::Url(text),
            ItemSource::File { bytes } => AddSource::TorrentFile(bytes.to_vec()),
        };
        Some(Work {
            key,
            job_id,
            timeout,
            fut: self.client.add_torrent(src, &opts),
        })
    }

    fn item_mut(&mut self, key: u64) -> Option<&mut StagedItem> {
        self.items.iter_mut().find(|i| i.key == key)
    }

    fn is_in_flight(&self, key: u64, job_id: &str) -> bool {
        self.items
            .iter()
            .any(|i| i.key == key && i.job.as_ref().is_some_and(|j| j.job_id == job_id))
    }

    /// Polls the server's view of the job and arms the client-side timeout.
    fn spawn_job_watchers(
        &mut self,
        key: u64,
        job_id: String,
        timeout: Duration,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        let poll_id = job_id.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(JOB_POLL_INTERVAL).await;
                let still = this
                    .update(cx, |p, _| p.is_in_flight(key, &poll_id))
                    .unwrap_or(false);
                if !still {
                    return;
                }
                // 404 until the request reaches the server.
                if let Ok(st) = cx
                    .background_executor()
                    .spawn(client.get_add_job(&poll_id))
                    .await
                {
                    this.update(cx, |p, cx| {
                        if let Some(item) = p.item_mut(key)
                            && item.job.as_ref().is_some_and(|j| j.job_id == poll_id)
                        {
                            item.stage_since = Some(
                                Instant::now()
                                    .checked_sub(Duration::from_secs_f64(st.stage_secs.max(0.0)))
                                    .unwrap_or_else(Instant::now),
                            );
                            item.stage = Some(st);
                            cx.notify();
                        }
                    })
                    .ok();
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(timeout).await;
            this.update(cx, |p, cx| {
                if let Some(item) = p.item_mut(key)
                    && let Some(job) = item.job.as_mut().filter(|j| j.job_id == job_id)
                {
                    job.timed_out = true;
                    p.cancel_in_flight(key, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Cancel one in-flight add on the server. If the server says it was
    /// already committed, the request is left to finish and the item says so.
    fn cancel_in_flight(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(item) = self.item_mut(key) else {
            return;
        };
        let Some(job) = item.job.as_mut() else { return };
        if job.cancel != CancelState::None {
            return;
        }
        job.cancel = CancelState::Pending;
        let job_id = job.job_id.clone();
        let client = self.client.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.cancel_add_job(&job_id))
                .await;
            this.update(cx, |p, cx| {
                let Some(item) = p.item_mut(key) else { return };
                let Some(job) = item.job.as_mut().filter(|j| j.job_id == job_id) else {
                    return;
                };
                match r {
                    Ok(o) if o.result == "already_added" => {
                        job.cancel = CancelState::AlreadyAdded;
                        item.added_id = o.torrent_id;
                        item.note = Some(format!(
                            "Too late to cancel: the server had already added it (id {}).",
                            o.torrent_id
                                .map(|i| i.to_string())
                                .unwrap_or_else(|| "?".into())
                        ));
                    }
                    // Finished some other way (failed / listed); let the request settle.
                    Ok(o) if o.result == "finished" => job.cancel = CancelState::None,
                    Ok(_) => job.cancel = CancelState::Cancelled,
                    Err(e) => {
                        // Can't reach the server to cancel; treat as cancelled
                        // and report if the add still went through.
                        job.cancel = CancelState::Cancelled;
                        item.note = Some(format!("Cancel request failed: {e:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn finish(
        &mut self,
        key: u64,
        job_id: &str,
        res: anyhow::Result<AddTorrentResponse>,
        final_stage: Option<AddJobStatus>,
        cx: &mut Context<Self>,
    ) {
        let mut added = false;
        if let Some(item) = self.item_mut(key)
            && item.job.as_ref().is_some_and(|j| j.job_id == job_id)
        {
            let job = item.job.take().expect("checked above");
            item.stage = None;
            item.stage_since = None;
            item.status = match (job.cancel, res) {
                (CancelState::Cancelled, Ok(r)) => {
                    item.added_id = r.id;
                    item.note = Some("Added although the add was cancelled.".into());
                    added = true;
                    ItemStatus::Added(r.id)
                }
                (CancelState::Cancelled, Err(_)) if job.timed_out => {
                    ItemStatus::Failed(timeout_error(job.magnet, job.timeout))
                }
                (CancelState::Cancelled, Err(_)) => ItemStatus::Cancelled,
                (cancel, Ok(r)) => {
                    if cancel == CancelState::AlreadyAdded {
                        item.added_id = r.id.or(item.added_id);
                    }
                    let fs = final_stage.as_ref();
                    if fs.is_some_and(|s| s.stage == "already_managed") {
                        ItemStatus::AlreadyInRqbit(r.id.or(fs.and_then(|s| s.torrent_id)))
                    } else {
                        added = true;
                        ItemStatus::Added(r.id)
                    }
                }
                (_, Err(e)) if job.timed_out => {
                    let _ = e;
                    ItemStatus::Failed(timeout_error(job.magnet, job.timeout))
                }
                (_, Err(e)) => ItemStatus::Failed(format!("{e:#}")),
            };
        }
        if added {
            self.batch_added += 1;
        }
        cx.notify();
    }

    fn stop_queue(&mut self, cx: &mut Context<Self>) {
        self.stop_requested = true;
        for i in &mut self.items {
            if i.status == ItemStatus::Queued {
                i.status = ItemStatus::Cancelled;
            }
        }
        let keys: Vec<u64> = self
            .items
            .iter()
            .filter(|i| i.job.is_some())
            .map(|i| i.key)
            .collect();
        for k in keys {
            self.cancel_in_flight(k, cx);
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.running {
            // The panel goes away: cancel what's in flight on the server
            // without waiting for answers.
            self.stop_requested = true;
            for i in &self.items {
                if let Some(j) = &i.job
                    && j.cancel == CancelState::None
                {
                    cx.background_executor()
                        .spawn(self.client.cancel_add_job(&j.job_id))
                        .detach();
                }
            }
        }
        cx.emit(AddPanelEvent::Close);
    }

    fn remove_from_rqbit(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(id) = self
            .items
            .iter()
            .find(|i| i.key == key)
            .and_then(|i| i.added_id)
        else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(client.forget(id)).await;
            this.update(cx, |p, cx| {
                if let Some(item) = p.item_mut(key) {
                    match r {
                        Ok(()) => {
                            item.status = ItemStatus::Cancelled;
                            item.added_id = None;
                            item.note = Some("Removed from rqbit (files kept).".into());
                        }
                        Err(e) => item.note = Some(format!("Remove failed: {e:#}")),
                    }
                }
                cx.emit(AddPanelEvent::Added);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_item(&self, item: &StagedItem, cx: &mut Context<Self>) -> gpui::AnyElement {
        let key = item.key;
        let running = self.running;
        let in_flight = item.status == ItemStatus::Running;
        let cancelling = item
            .job
            .as_ref()
            .is_some_and(|j| j.cancel == CancelState::Pending);
        let (status_text, status_color) = match &item.status {
            ItemStatus::Ready => ("ready".to_owned(), theme::text_muted()),
            ItemStatus::Queued => ("queued".to_owned(), theme::text_muted()),
            ItemStatus::Running => {
                let label = if cancelling {
                    "cancelling…".to_owned()
                } else {
                    stage_label(item.stage.as_ref())
                };
                let since = item.stage_since.or(item.started);
                let elapsed = since
                    .map(|s| format!(" {}", format_elapsed(s.elapsed())))
                    .unwrap_or_default();
                (format!("{label}{elapsed}"), theme::primary())
            }
            ItemStatus::Added(id) => (
                match id {
                    Some(id) => format!("added (id {id})"),
                    None => "added".into(),
                },
                theme::success(),
            ),
            ItemStatus::AlreadyInRqbit(id) => (
                match id {
                    Some(id) => format!("already in rqbit (id {id})"),
                    None => "already in rqbit".into(),
                },
                theme::warning(),
            ),
            ItemStatus::Failed(_) => ("failed".to_owned(), theme::error()),
            ItemStatus::Cancelled => ("cancelled".to_owned(), theme::text_muted()),
        };
        let resolving = in_flight
            && !cancelling
            && item
                .stage
                .as_ref()
                .is_some_and(|s| s.stage == "resolving_metadata");
        let error = match &item.status {
            ItemStatus::Failed(e) => Some(e.clone()),
            _ => None,
        };
        let can_retry = matches!(item.status, ItemStatus::Failed(_) | ItemStatus::Cancelled)
            && item.source.is_some()
            && !running;

        div()
            .id(("staged", key as usize))
            .flex()
            .flex_row()
            .items_start()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text())
                            .truncate()
                            .child(item.label.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_x_2()
                            .text_xs()
                            .child(
                                div()
                                    .text_color(theme::text_muted())
                                    .child(item.origin.clone()),
                            )
                            .child(div().text_color(status_color).child(status_text))
                            .when(resolving, |d| {
                                d.child(
                                    div()
                                        .text_color(theme::text_muted())
                                        .child("waiting for peers to send torrent info"),
                                )
                            })
                            .when_some(item.note.clone(), |d, n| {
                                d.child(div().text_color(theme::warning()).child(n))
                            })
                            .when(item.added_id.is_some(), |d| {
                                d.child(
                                    widgets::link(
                                        ("rm-rqbit", key as usize),
                                        "remove from rqbit (keep files)",
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| this.remove_from_rqbit(key, cx),
                                    )),
                                )
                            })
                            .when_some(error, |d, e| {
                                d.child(div().text_color(theme::error()).child(e))
                            })
                            .when(can_retry, |d| {
                                d.child(widgets::link(("retry", key as usize), "retry").on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        if let Some(i) = this.item_mut(key) {
                                            i.status = ItemStatus::Ready;
                                            i.note = None;
                                            i.started = None;
                                        }
                                        cx.notify();
                                    }),
                                ))
                            }),
                    ),
            )
            .child(if in_flight {
                widgets::button(("cancel-item", key as usize), "×", !cancelling)
                    .when(!cancelling, |b| {
                        b.on_click(
                            cx.listener(move |this, _, _, cx| this.cancel_in_flight(key, cx)),
                        )
                    })
                    .tooltip(widgets::text_tooltip("Cancel this add".into()))
            } else {
                widgets::button(
                    ("remove-item", key as usize),
                    "×",
                    item.status != ItemStatus::Queued,
                )
                .when(item.status != ItemStatus::Queued, |b| {
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        this.items.retain(|i| i.key != key);
                        cx.notify();
                    }))
                })
                .tooltip(widgets::text_tooltip("Remove from this list".into()))
            })
            .into_any_element()
    }
}

fn section_label(text: &'static str) -> impl IntoElement {
    div()
        .text_sm()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme::text())
        .child(text)
}

impl Render for AddPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running;
        let pasted = self.pasted_not_queued(cx);
        let url_text_empty = self.urls.read(cx).text().trim().is_empty();
        let ready_count = self.startable() + pasted.len();
        let can_start = !running && ready_count > 0;

        let hint = if url_text_empty {
            "Paste one link, or several (one per line).".to_owned()
        } else if pasted.is_empty() {
            "No new magnets or torrent URLs detected.".to_owned()
        } else {
            format!(
                "{} source{} detected — press Add to start, or stage them for review.",
                pasted.len(),
                if pasted.len() == 1 { "" } else { "s" }
            )
        };

        let error_count = self
            .items
            .iter()
            .filter(|i| matches!(i.status, ItemStatus::Failed(_)))
            .count();
        let staged_ready = self
            .items
            .iter()
            .filter(|i| matches!(i.status, ItemStatus::Ready | ItemStatus::Queued))
            .count();

        let batch: Vec<&StagedItem> = self.items.iter().filter(|i| i.in_batch).collect();
        let progress = (!batch.is_empty()).then(|| {
            let total = batch.len();
            let done = batch.iter().filter(|i| i.status.is_terminal()).count();
            let count =
                |f: &dyn Fn(&ItemStatus) -> bool| batch.iter().filter(|i| f(&i.status)).count();
            let added = count(&|s| matches!(s, ItemStatus::Added(_)));
            let already = count(&|s| matches!(s, ItemStatus::AlreadyInRqbit(_)));
            let failed = count(&|s| matches!(s, ItemStatus::Failed(_)));
            let cancelled = count(&|s| matches!(s, ItemStatus::Cancelled));
            let in_flight: Vec<&&StagedItem> = batch
                .iter()
                .filter(|i| i.status == ItemStatus::Running)
                .collect();
            let resolving = in_flight
                .iter()
                .filter(|i| {
                    i.stage
                        .as_ref()
                        .is_some_and(|s| s.stage == "resolving_metadata")
                })
                .count();
            let waiting = in_flight
                .iter()
                .filter(|i| {
                    i.stage
                        .as_ref()
                        .is_some_and(|s| s.stage == "waiting_for_server")
                })
                .count();
            let adding = in_flight.len() - resolving - waiting;
            let parts: Vec<String> = [
                Some(format!("{done}/{total} done")),
                (resolving > 0).then(|| format!("{resolving} resolving metadata")),
                (waiting > 0).then(|| format!("{waiting} waiting for server (busy checking)")),
                (adding > 0).then(|| format!("{adding} adding")),
                Some(format!("{added} added")),
                (already > 0).then(|| format!("{already} already in rqbit")),
                (failed > 0).then(|| format!("{failed} failed")),
                (cancelled > 0).then(|| format!("{cancelled} cancelled")),
            ]
            .into_iter()
            .flatten()
            .collect();
            let frac = if total > 0 {
                done as f32 / total as f32
            } else {
                0.0
            };
            let color = if failed > 0 {
                theme::warning()
            } else if done >= total {
                theme::success()
            } else {
                theme::primary()
            };
            (parts.join(" · "), frac, color)
        });

        let output_set = !self.output_folder.read(cx).text().trim().is_empty();
        let summary: Vec<String> = [
            output_set.then(|| "custom output folder".to_owned()),
            (!self.overwrite).then(|| "no overwrite".to_owned()),
            (self.concurrency != DEFAULT_CONCURRENCY)
                .then(|| format!("{} at once", self.concurrency)),
        ]
        .into_iter()
        .flatten()
        .collect();

        let mut item_rows = Vec::with_capacity(self.items.len());
        for i in &self.items {
            item_rows.push(self.render_item(i, cx));
        }

        let drop_hint = if cfg!(target_family = "wasm") {
            "…or drop .torrent files anywhere on the page."
        } else {
            "…or drop .torrent files onto the window."
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Add torrents"),
            )
            .child(div().flex_1())
            .child(
                widgets::button("add-close-x", "×", !running).when(!running, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| this.close(cx)))
                }),
            );

        let body = div()
            .id("add-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .gap_3()
            .p_4()
            .child(section_label("Magnet links / torrent URLs (one per line)"))
            .child(self.urls.clone())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().text_xs().text_color(theme::text_muted()).child(hint))
                    .child(
                        widgets::button("stage-urls", "Stage for review", !running && !pasted.is_empty())
                            .when(!running && !pasted.is_empty(), |b| {
                                b.on_click(cx.listener(|this, _, _, cx| {
                                    this.stage_pasted(cx);
                                    cx.notify();
                                }))
                            }),
                    ),
            )
            .child(section_label("Torrent files"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        widgets::button("choose-files", "Choose .torrent files…", !running)
                            .when(!running, |b| b.on_click(cx.listener(|this, _, _, cx| this.choose_files(cx)))),
                    )
                    .child(div().text_xs().text_color(theme::text_muted()).child(drop_hint)),
            )
            .when_some(self.message.clone(), |d, m| {
                d.child(div().text_xs().text_color(theme::text_muted()).child(m))
            })
            .when(!self.items.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div().flex_1().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child(format!(
                                "Queue · {} item{}{}{}",
                                self.items.len(),
                                if self.items.len() == 1 { "" } else { "s" },
                                if staged_ready > 0 { format!(" · {staged_ready} ready") } else { String::new() },
                                if error_count > 0 { format!(" · {error_count} error{}", if error_count == 1 { "" } else { "s" }) } else { String::new() },
                            )),
                        )
                        .when(error_count > 0, |d| {
                            d.child(
                                widgets::button("dismiss-errors", "Dismiss errors", !running).when(!running, |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| {
                                        this.items.retain(|i| !matches!(i.status, ItemStatus::Failed(_)));
                                        cx.notify();
                                    }))
                                }),
                            )
                        })
                        .child(widgets::button("clear-all", "Clear all", !running).when(!running, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| {
                                this.items.clear();
                                cx.notify();
                            }))
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::border())
                        .children(item_rows),
                )
            })
            .child(
                div()
                    .id("advanced-toggle")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme::text()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.advanced_open = !this.advanced_open;
                        cx.notify();
                    }))
                    .child(if self.advanced_open { "−" } else { "+" })
                    .child("Advanced")
                    .when(!self.advanced_open && !summary.is_empty(), |d| {
                        d.child(div().text_color(theme::primary()).child(format!("· {}", summary.join(" · "))))
                    }),
            )
            .when(self.advanced_open, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .pl_3()
                        .border_l_2()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_sm().child("Output folder (optional)"))
                                .child(div().max_w(px(520.)).child(self.output_folder.clone())),
                        )
                        .child(
                            widgets::checkbox("overwrite", "Overwrite existing files on disk", self.overwrite, !running)
                                .when(!running, |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| {
                                        this.overwrite = !this.overwrite;
                                        cx.notify();
                                    }))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .child(format!("Concurrent adds: {}", self.concurrency))
                                .child(widgets::button("conc-dec", "−", !running && self.concurrency > 2).when(
                                    !running && self.concurrency > 2,
                                    |b| {
                                        b.on_click(cx.listener(|this, _, _, cx| {
                                            this.concurrency -= 1;
                                            cx.notify();
                                        }))
                                    },
                                ))
                                .child(widgets::button("conc-inc", "+", !running && self.concurrency < 8).when(
                                    !running && self.concurrency < 8,
                                    |b| {
                                        b.on_click(cx.listener(|this, _, _, cx| {
                                            this.concurrency += 1;
                                            cx.notify();
                                        }))
                                    },
                                )),
                        )
                        .child(div().text_xs().text_color(theme::text_muted()).child(
                            "Browse server, Transfer from other client and .zip unpacking are in the web UI's Add dialog.",
                        )),
                )
            })
            .when_some(progress, |d, (text, frac, color)| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xs().text_color(theme::text_muted()).child(text))
                        .child(
                            div()
                                .h(px(6.))
                                .w_full()
                                .rounded_full()
                                .bg(theme::border())
                                .child(div().h_full().rounded_full().bg(color).w(gpui::relative(frac))),
                        ),
                )
            });

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap_2()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(theme::border())
            .child(if running {
                widgets::danger_button("stop-queue", "Stop queue")
                    .on_click(cx.listener(|this, _, _, cx| this.stop_queue(cx)))
            } else {
                widgets::button("add-close", "Close", true)
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            })
            .child(
                widgets::primary_button(
                    "add-start",
                    if running {
                        "Adding…".to_owned()
                    } else if ready_count > 0 {
                        format!("Add {ready_count}")
                    } else {
                        "Add".to_owned()
                    },
                    can_start,
                )
                .when(can_start, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| this.start_import(cx)))
                }),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(body)
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_ids_are_unique_and_url_safe() {
        let a = new_job_id();
        let b = new_job_id();
        assert_ne!(a, b);
        assert!(a.starts_with("add-gpui-"));
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'));
    }

    #[test]
    fn labels() {
        assert_eq!(format_elapsed(Duration::from_secs(75)), "1:15");
        assert_eq!(format_duration(MAGNET_TIMEOUT), "3 min");
        assert_eq!(stage_label(None), "sending to server…");
        let st = AddJobStatus {
            stage: "adding".into(),
            step: Some("saving".into()),
            busy: Some(true),
            ..Default::default()
        };
        assert_eq!(
            stage_label(Some(&st)),
            "adding: saving (server busy checking other torrents)…"
        );
        assert!(timeout_error(true, MAGNET_TIMEOUT).contains("3 min"));
    }
}
