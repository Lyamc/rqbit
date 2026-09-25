//! Server-side tracking of in-flight "add torrent" requests.
//!
//! A client may pass `add_job_id` with an add request. While it runs, the job
//! reports what the server is actually doing (resolving magnet metadata,
//! waiting for a disk slot behind hash checks, ...), and it can be cancelled.
//!
//! Cancellation is decided atomically against the commit point (the moment
//! the torrent is inserted into the session): a cancel either happens before
//! it (nothing is added) or after it (the caller is told it was already
//! added). Dropping the add future (e.g. the HTTP client disconnected) cancels
//! the job the same way; the commit itself runs to completion in its own task
//! so a torrent is never left half-added (in memory but not persisted).

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::bail;
use parking_lot::Mutex;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::adopt::AdoptSummary;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum AddJobStage {
    Starting,
    /// Downloading the .torrent from an http(s) URL.
    FetchingTorrent,
    /// Magnet: waiting for peers to send the torrent metadata.
    ResolvingMetadata,
    /// Inspecting / renaming another client's files.
    Adopting,
    /// Waiting for a disk I/O slot (other torrents are being hash-checked).
    WaitingForServer,
    /// Committed: creating files and persisting. Can no longer be cancelled.
    Adding { torrent_id: usize },
    Added { torrent_id: usize },
    AlreadyManaged { torrent_id: usize },
    ListOnly,
    Failed { error: String },
    Cancelled { reason: String },
}

impl AddJobStage {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            AddJobStage::Added { .. }
                | AddJobStage::AlreadyManaged { .. }
                | AddJobStage::ListOnly
                | AddJobStage::Failed { .. }
                | AddJobStage::Cancelled { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AddJobStatus {
    pub job_id: Option<String>,
    #[serde(flatten)]
    pub stage: AddJobStage,
    /// Seconds spent in the current stage.
    pub stage_secs: f64,
    pub elapsed_secs: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adopt: Option<AdoptSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum AddJobCancelOutcome {
    /// Cancelled before the torrent was committed: nothing will be added.
    Cancelled,
    /// Too late: the torrent was already committed / added.
    AlreadyAdded { torrent_id: usize },
    /// The job had already finished another way.
    Finished {
        #[serde(flatten)]
        stage: AddJobStage,
    },
}

struct Inner {
    stage: AddJobStage,
    committed: Option<usize>,
    created: Instant,
    stage_since: Instant,
    finished_at: Option<Instant>,
    adopt: Option<AdoptSummary>,
}

pub struct AddJob {
    pub id: Option<String>,
    inner: Mutex<Inner>,
    token: CancellationToken,
}

impl AddJob {
    pub fn new(id: Option<String>) -> Arc<Self> {
        let now = Instant::now();
        Arc::new(Self {
            id,
            inner: Mutex::new(Inner {
                stage: AddJobStage::Starting,
                committed: None,
                created: now,
                stage_since: now,
                finished_at: None,
                adopt: None,
            }),
            token: CancellationToken::new(),
        })
    }

    /// Update the reported stage (ignored once committed or finished).
    pub fn set_stage(&self, stage: AddJobStage) {
        let mut g = self.inner.lock();
        if g.stage.is_terminal() || g.committed.is_some() {
            return;
        }
        if g.stage != stage {
            g.stage = stage;
            g.stage_since = Instant::now();
        }
    }

    pub fn set_adopt_summary(&self, s: AdoptSummary) {
        self.inner.lock().adopt = Some(s);
    }

    /// The commit point. Returns false if the job was cancelled first.
    pub fn try_commit(&self, torrent_id: usize) -> bool {
        let mut g = self.inner.lock();
        if self.token.is_cancelled() || g.stage.is_terminal() {
            return false;
        }
        g.committed = Some(torrent_id);
        g.stage = AddJobStage::Adding { torrent_id };
        g.stage_since = Instant::now();
        true
    }

    pub fn cancel(&self, reason: &str) -> AddJobCancelOutcome {
        let mut g = self.inner.lock();
        if let Some(torrent_id) = g.committed {
            return match &g.stage {
                AddJobStage::Failed { .. } => AddJobCancelOutcome::Finished {
                    stage: g.stage.clone(),
                },
                _ => AddJobCancelOutcome::AlreadyAdded { torrent_id },
            };
        }
        if g.stage.is_terminal() {
            return match &g.stage {
                AddJobStage::Cancelled { .. } => AddJobCancelOutcome::Cancelled,
                AddJobStage::AlreadyManaged { torrent_id } => AddJobCancelOutcome::AlreadyAdded {
                    torrent_id: *torrent_id,
                },
                s => AddJobCancelOutcome::Finished { stage: s.clone() },
            };
        }
        g.stage = AddJobStage::Cancelled {
            reason: reason.to_owned(),
        };
        g.stage_since = Instant::now();
        g.finished_at = Some(Instant::now());
        self.token.cancel();
        AddJobCancelOutcome::Cancelled
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Run `fut` unless/until the job is cancelled.
    pub async fn cancellable<T>(
        &self,
        fut: impl std::future::Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        tokio::select! {
            biased;
            _ = self.token.cancelled() => bail!("add cancelled"),
            r = fut => r,
        }
    }

    pub fn bail_if_cancelled(&self) -> anyhow::Result<()> {
        if self.token.is_cancelled() {
            bail!("add cancelled");
        }
        Ok(())
    }

    pub fn finish(&self, stage: AddJobStage) {
        let mut g = self.inner.lock();
        if matches!(g.stage, AddJobStage::Cancelled { .. }) && g.committed.is_none() {
            return;
        }
        g.stage = stage;
        g.stage_since = Instant::now();
        g.finished_at = Some(Instant::now());
    }

    pub fn status(&self) -> AddJobStatus {
        let g = self.inner.lock();
        AddJobStatus {
            job_id: self.id.clone(),
            stage: g.stage.clone(),
            stage_secs: g.stage_since.elapsed().as_secs_f64(),
            elapsed_secs: g.created.elapsed().as_secs_f64(),
            adopt: g.adopt.clone(),
        }
    }

    fn finished_longer_than(&self, d: Duration) -> bool {
        self.inner
            .lock()
            .finished_at
            .is_some_and(|f| f.elapsed() > d)
    }
}

/// Cancels the job if dropped before it was committed (client went away).
pub(crate) struct CancelOnDrop(pub Arc<AddJob>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let AddJobCancelOutcome::Cancelled =
            self.0.cancel("add request dropped (client disconnected or timed out)")
        {
            tracing::debug!(job_id = ?self.0.id, "add job cancelled because its request was dropped");
        }
    }
}

const KEEP_FINISHED: Duration = Duration::from_secs(15 * 60);
const MAX_JOB_ID_LEN: usize = 128;

#[derive(Default)]
pub struct AddJobs {
    jobs: Mutex<HashMap<String, Arc<AddJob>>>,
}

impl AddJobs {
    fn prune(map: &mut HashMap<String, Arc<AddJob>>) {
        map.retain(|_, j| !j.finished_longer_than(KEEP_FINISHED));
    }

    fn validate(id: &str) -> anyhow::Result<()> {
        if id.is_empty()
            || id.len() > MAX_JOB_ID_LEN
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("invalid add_job_id (use 1-128 of [A-Za-z0-9_-])");
        }
        Ok(())
    }

    /// Register a new job. Fails if the id was cancelled before the request
    /// arrived, or is already in use.
    pub fn register(&self, id: String) -> anyhow::Result<Arc<AddJob>> {
        Self::validate(&id)?;
        let mut g = self.jobs.lock();
        Self::prune(&mut g);
        if let Some(existing) = g.get(&id) {
            if existing.is_cancelled() {
                bail!("add cancelled (cancel arrived before the request)");
            }
            bail!("add_job_id {id:?} is already in use");
        }
        let job = AddJob::new(Some(id.clone()));
        g.insert(id, job.clone());
        Ok(job)
    }

    pub fn get(&self, id: &str) -> Option<AddJobStatus> {
        self.jobs.lock().get(id).map(|j| j.status())
    }

    /// Cancel a job. An unknown id leaves a cancelled tombstone so a request
    /// that arrives later with this id is refused.
    pub fn cancel(&self, id: &str) -> anyhow::Result<AddJobCancelOutcome> {
        Self::validate(id)?;
        let mut g = self.jobs.lock();
        Self::prune(&mut g);
        if let Some(j) = g.get(id) {
            return Ok(j.cancel("cancelled by user"));
        }
        let j = AddJob::new(Some(id.to_owned()));
        let r = j.cancel("cancelled by user before the request arrived");
        g.insert(id.to_owned(), j);
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_before_commit_blocks_commit() {
        let j = AddJob::new(None);
        j.set_stage(AddJobStage::WaitingForServer);
        assert!(matches!(j.cancel("x"), AddJobCancelOutcome::Cancelled));
        assert!(!j.try_commit(5));
        assert!(matches!(j.status().stage, AddJobStage::Cancelled { .. }));
    }

    #[test]
    fn cancel_after_commit_reports_added() {
        let j = AddJob::new(None);
        assert!(j.try_commit(7));
        assert!(matches!(
            j.cancel("x"),
            AddJobCancelOutcome::AlreadyAdded { torrent_id: 7 }
        ));
        assert!(!j.is_cancelled());
        j.finish(AddJobStage::Added { torrent_id: 7 });
        assert_eq!(j.status().stage, AddJobStage::Added { torrent_id: 7 });
    }

    #[test]
    fn cancel_on_drop_only_before_commit() {
        let j = AddJob::new(None);
        drop(CancelOnDrop(j.clone()));
        assert!(j.is_cancelled());

        let j = AddJob::new(None);
        assert!(j.try_commit(1));
        drop(CancelOnDrop(j.clone()));
        assert!(!j.is_cancelled());
    }

    #[test]
    fn tombstone_refuses_late_request() {
        let jobs = AddJobs::default();
        assert!(matches!(
            jobs.cancel("abc").unwrap(),
            AddJobCancelOutcome::Cancelled
        ));
        assert!(jobs.register("abc".into()).is_err());
        assert!(jobs.register("def".into()).is_ok());
        assert!(jobs.register("def".into()).is_err());
        assert!(jobs.register("bad id!".into()).is_err());
    }
}
