//! "Start after I finish the Add dialog": torrents added from an Add dialog are held
//! paused while it is open and started when it closes.
//!
//! The UI sends a dialog id (`add_dialog_id`, any 1-128 of `[A-Za-z0-9_-]`) with each
//! add. When the preference is on and the add wasn't paused on purpose, the torrent
//! goes in paused and its id is recorded under the dialog. The dialog then:
//! * sends `POST /add_dialog/{id}/heartbeat` every ~15 s while open;
//! * sends `POST /add_dialog/{id}/finish` when it closes (button, Escape, auto-close
//!   after success; on tab/window close via `navigator.sendBeacon`), which starts
//!   every held torrent that is still there and still paused.
//!
//! Fallback: a dialog that stops sending heartbeats for [`DIALOG_TIMEOUT`] (tab
//! killed, beacon lost, GPUI app quit) is finished by the server. Holds are persisted
//! (`add-dialog-holds.json`); after a restart the dialog is gone, so they are started
//! at startup. Removed torrents are simply skipped.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::Session;

/// No heartbeat for this long: the dialog is considered closed. Browsers throttle
/// timers in background tabs to about once a minute, so a 15 s heartbeat can arrive
/// ~60 s apart; this leaves room for that.
pub const DIALOG_TIMEOUT: Duration = Duration::from_secs(150);
const REAP_INTERVAL: Duration = Duration::from_secs(10);

struct Hold {
    ids: Vec<usize>,
    last_seen: Instant,
}

#[derive(Default, Serialize, Deserialize)]
struct HoldFile {
    #[serde(default)]
    holds: HashMap<String, Vec<usize>>,
}

#[derive(Default)]
pub struct AddDialogs {
    path: Option<PathBuf>,
    holds: Mutex<HashMap<String, Hold>>,
    /// Loaded from disk at startup: started once the session is up.
    stale: Mutex<Vec<usize>>,
    save_lock: Mutex<()>,
}

#[derive(Debug, Default, Serialize)]
pub struct DialogRelease {
    /// Torrents started (or, for magnets still resolving, set to start once their
    /// metadata arrives).
    pub started: Vec<usize>,
    /// Already running (started by hand meanwhile).
    pub already_running: Vec<usize>,
    /// Removed while the dialog was open.
    pub gone: Vec<usize>,
}

pub fn validate_dialog_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        anyhow::bail!("invalid add_dialog_id (use 1-128 of [A-Za-z0-9_-])");
    }
    Ok(())
}

impl AddDialogs {
    pub fn load(path: PathBuf) -> Self {
        let stale: Vec<usize> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<HoldFile>(&b).ok())
            .map(|f| f.holds.into_values().flatten().collect())
            .unwrap_or_default();
        Self {
            path: Some(path),
            holds: Default::default(),
            stale: Mutex::new(stale),
            save_lock: Default::default(),
        }
    }

    fn save(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let _g = self.save_lock.lock();
        let f = HoldFile {
            holds: self
                .holds
                .lock()
                .iter()
                .map(|(k, h)| (k.clone(), h.ids.clone()))
                .chain(
                    // Not started yet (startup): keep them on disk until they are.
                    Some(("stale".to_owned(), self.stale.lock().clone()))
                        .filter(|(_, v)| !v.is_empty()),
                )
                .collect(),
        };
        let res = (|| -> anyhow::Result<()> {
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&f)?)?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        })();
        if let Err(e) = res {
            warn!(?path, "error saving add dialog holds: {e:#}");
        }
    }

    /// Record a torrent held for `dialog`.
    pub fn hold(&self, dialog: &str, id: usize) {
        {
            let mut g = self.holds.lock();
            let h = g.entry(dialog.to_owned()).or_insert_with(|| Hold {
                ids: Vec::new(),
                last_seen: Instant::now(),
            });
            h.last_seen = Instant::now();
            if !h.ids.contains(&id) {
                h.ids.push(id);
            }
        }
        self.save();
    }

    /// Dialog still open. Returns the ids held for it.
    pub fn heartbeat(&self, dialog: &str) -> Vec<usize> {
        let mut g = self.holds.lock();
        match g.get_mut(dialog) {
            Some(h) => {
                h.last_seen = Instant::now();
                h.ids.clone()
            }
            None => Vec::new(),
        }
    }

    fn take(&self, dialog: &str) -> Vec<usize> {
        let r = self.holds.lock().remove(dialog).map(|h| h.ids);
        if r.is_some() {
            self.save();
        }
        r.unwrap_or_default()
    }

    fn take_expired(&self, timeout: Duration) -> Vec<(String, Vec<usize>)> {
        let expired: Vec<(String, Vec<usize>)> = {
            let mut g = self.holds.lock();
            let keys: Vec<String> = g
                .iter()
                .filter(|(_, h)| h.last_seen.elapsed() > timeout)
                .map(|(k, _)| k.clone())
                .collect();
            keys.into_iter()
                .filter_map(|k| g.remove(&k).map(|h| (k, h.ids)))
                .collect()
        };
        if !expired.is_empty() {
            self.save();
        }
        expired
    }

    pub fn held_count(&self) -> usize {
        self.holds.lock().values().map(|h| h.ids.len()).sum()
    }
}

impl Session {
    /// Start what an Add dialog held (dialog finished / timed out / server restart).
    pub async fn release_add_dialog(self: &Arc<Self>, dialog: &str, why: &str) -> DialogRelease {
        let ids = self.add_dialogs.take(dialog);
        self.start_held(ids, dialog, why).await
    }

    async fn start_held(self: &Arc<Self>, ids: Vec<usize>, dialog: &str, why: &str) -> DialogRelease {
        let mut r = DialogRelease::default();
        for id in ids {
            if let Some(pm) = self.pending.get(id) {
                if pm.paused {
                    self.pending_unpause_quiet(id);
                    r.started.push(id);
                } else {
                    r.already_running.push(id);
                }
                continue;
            }
            let handle = self.db.read().torrents.get(&id).cloned();
            match handle {
                Some(h) if h.is_paused() => match self.unpause(&h).await {
                    Ok(()) => r.started.push(id),
                    Err(e) => warn!(id, "error starting torrent held by Add dialog: {e:#}"),
                },
                Some(_) => r.already_running.push(id),
                None => r.gone.push(id),
            }
        }
        if !(r.started.is_empty() && r.already_running.is_empty() && r.gone.is_empty()) {
            info!(
                dialog,
                why,
                started = ?r.started,
                already_running = ?r.already_running,
                gone = ?r.gone,
                "Add dialog finished: started the torrents it held"
            );
        }
        r
    }

    /// Start holds left from before a restart, then finish dialogs that stopped
    /// sending heartbeats.
    pub(crate) fn start_add_dialog_reaper(self: &Arc<Self>) {
        let this = Arc::downgrade(self);
        tokio::spawn(
            async move {
                let stale = match this.upgrade() {
                    Some(s) => std::mem::take(&mut *s.add_dialogs.stale.lock()),
                    None => return,
                };
                if !stale.is_empty()
                    && let Some(s) = this.upgrade()
                {
                    // Let restored torrents finish initializing first.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    s.start_held(stale, "(before restart)", "server restarted while an Add dialog was open")
                        .await;
                    s.add_dialogs.save();
                }
                loop {
                    tokio::time::sleep(REAP_INTERVAL).await;
                    let Some(s) = this.upgrade() else {
                        return;
                    };
                    for (dialog, ids) in s.add_dialogs.take_expired(dialog_timeout()) {
                        s.start_held(ids, &dialog, "no heartbeat from the Add dialog").await;
                    }
                }
            },
        );
    }
}

/// `RQBIT_ADD_DIALOG_TIMEOUT_SECS` overrides [`DIALOG_TIMEOUT`] (testing).
fn dialog_timeout() -> Duration {
    std::env::var("RQBIT_ADD_DIALOG_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DIALOG_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_persist_and_expire() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("add-dialog-holds.json");
        let d = AddDialogs::load(p.clone());
        d.hold("dlg-a", 3);
        d.hold("dlg-a", 4);
        d.hold("dlg-a", 3);
        d.hold("dlg-b", 9);
        assert_eq!(d.heartbeat("dlg-a"), vec![3, 4]);
        assert_eq!(d.held_count(), 3);
        // Survives a restart as "stale" (started at startup).
        let d2 = AddDialogs::load(p);
        let mut stale = d2.stale.lock().clone();
        stale.sort();
        assert_eq!(stale, vec![3, 4, 9]);
        // Expiry.
        assert!(d.take_expired(Duration::from_secs(3600)).is_empty());
        let mut e = d.take_expired(Duration::ZERO);
        e.sort();
        assert_eq!(e.len(), 2);
        assert_eq!(d.held_count(), 0);
        assert!(d.take("dlg-a").is_empty());
        assert!(validate_dialog_id("add-web-1_x").is_ok());
        assert!(validate_dialog_id("bad id").is_err());
        assert!(validate_dialog_id("").is_err());
    }
}
