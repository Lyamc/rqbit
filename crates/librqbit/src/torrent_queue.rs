//! qBittorrent-style queueing: limits on active downloads / uploads / torrents.
//!
//! Off by default. When enabled, torrents over the limits are *held* (kept paused
//! internally, not user-paused) in queue order and started when a slot frees up. Queue
//! order is persisted in `queue.json` next to `preferences.json`.

use std::{
    str::FromStr,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use librqbit_core::hash_id::Id20;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::session::TorrentId;

/// "Slow" torrents (for "don't count slow torrents"): below this rate both ways...
pub const SLOW_RATE_BYTES_PER_SEC: u64 = 2 * 1024;
/// ...for at least this long.
pub const SLOW_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueLimits {
    /// None = unlimited.
    pub max_downloads: Option<u32>,
    pub max_uploads: Option<u32>,
    pub max_active: Option<u32>,
    /// Slow active torrents keep running and don't count toward the limits.
    pub ignore_slow: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueCandidate {
    pub id: TorrentId,
    /// Finished (would seed) vs. still downloading.
    pub seeding: bool,
    /// Currently running.
    pub active: bool,
    /// Running and slow for at least [`SLOW_AFTER`].
    pub slow: bool,
}

/// Which candidates may run. `cands` must be in queue order.
pub fn admit(cands: &[QueueCandidate], l: &QueueLimits) -> HashSet<TorrentId> {
    let under = |n: u32, lim: Option<u32>| lim.map(|m| n < m).unwrap_or(true);
    let (mut dl, mut up, mut total) = (0u32, 0u32, 0u32);
    let mut out = HashSet::new();
    for c in cands {
        let exempt = l.ignore_slow && c.active && c.slow;
        let fits = if c.seeding {
            under(up, l.max_uploads)
        } else {
            under(dl, l.max_downloads)
        } && under(total, l.max_active);
        if exempt {
            out.insert(c.id);
            continue;
        }
        if fits {
            out.insert(c.id);
            if c.seeding {
                up += 1;
            } else {
                dl += 1;
            }
            total += 1;
        }
    }
    out
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueMove {
    Up,
    Down,
    Top,
    Bottom,
}

/// Move the selected entries (multi-select, relative order preserved).
pub fn apply_move<T: Copy + Eq + std::hash::Hash>(order: &mut [T], sel: &HashSet<T>, mv: QueueMove) {
    let n = order.len();
    match mv {
        QueueMove::Up => {
            for i in 1..n {
                if sel.contains(&order[i]) && !sel.contains(&order[i - 1]) {
                    order.swap(i - 1, i);
                }
            }
        }
        QueueMove::Down => {
            for i in (0..n.saturating_sub(1)).rev() {
                if sel.contains(&order[i]) && !sel.contains(&order[i + 1]) {
                    order.swap(i, i + 1);
                }
            }
        }
        QueueMove::Top | QueueMove::Bottom => {
            let (mut a, mut b): (Vec<T>, Vec<T>) = order.iter().partition(|x| sel.contains(x));
            let v: Vec<T> = if mv == QueueMove::Top {
                a.append(&mut b);
                a
            } else {
                b.append(&mut a);
                b
            };
            order.copy_from_slice(&v);
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct QueueFile {
    /// Info hashes (hex), queue order.
    order: Vec<String>,
}

#[derive(Default)]
pub struct TorrentQueue {
    path: Option<PathBuf>,
    order: Mutex<Vec<Id20>>,
    admitted: Mutex<HashSet<TorrentId>>,
    slow_since: Mutex<HashMap<TorrentId, Instant>>,
    pub(crate) kick: tokio::sync::Notify,
}

impl TorrentQueue {
    pub fn load(path: PathBuf) -> Self {
        let order = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<QueueFile>(&b).ok())
            .map(|f| {
                f.order
                    .iter()
                    .filter_map(|h| Id20::from_str(h).ok())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            path: Some(path),
            order: Mutex::new(order),
            ..Default::default()
        }
    }

    fn save(&self, order: &[Id20]) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if let Err(e) = write_atomic(path, order) {
            warn!(?path, "error saving queue order: {e:#}");
        }
    }

    /// Keep the order in sync with the torrents in the session: append new ones (by id),
    /// drop removed ones.
    pub fn sync(&self, present: &[(TorrentId, Id20)]) {
        let mut g = self.order.lock();
        let set: HashSet<Id20> = present.iter().map(|(_, h)| *h).collect();
        let before = g.len();
        g.retain(|h| set.contains(h));
        let mut changed = g.len() != before;
        let known: HashSet<Id20> = g.iter().copied().collect();
        let mut new: Vec<(TorrentId, Id20)> = present
            .iter()
            .filter(|(_, h)| !known.contains(h))
            .copied()
            .collect();
        new.sort_by_key(|(id, _)| *id);
        if !new.is_empty() {
            changed = true;
            g.extend(new.into_iter().map(|(_, h)| h));
        }
        if changed {
            self.save(&g);
        }
    }

    /// 1-based queue position.
    pub fn position(&self, h: &Id20) -> Option<usize> {
        self.order.lock().iter().position(|x| x == h).map(|p| p + 1)
    }

    pub fn order(&self) -> Vec<Id20> {
        self.order.lock().clone()
    }

    pub fn move_hashes(&self, sel: &HashSet<Id20>, mv: QueueMove) {
        let mut g = self.order.lock();
        apply_move(&mut g, sel, mv);
        self.save(&g);
        drop(g);
        self.kick.notify_one();
    }

    pub fn is_admitted(&self, id: TorrentId) -> bool {
        self.admitted.lock().contains(&id)
    }

    pub(crate) fn set_admitted(&self, s: HashSet<TorrentId>) {
        *self.admitted.lock() = s;
    }

    pub(crate) fn admit_one(&self, id: TorrentId) {
        self.admitted.lock().insert(id);
    }

    /// Track slowness of an active torrent; returns whether it counts as slow.
    pub(crate) fn observe_speed(&self, id: TorrentId, down_bps: u64, up_bps: u64, now: Instant) -> bool {
        let mut g = self.slow_since.lock();
        if down_bps < SLOW_RATE_BYTES_PER_SEC && up_bps < SLOW_RATE_BYTES_PER_SEC {
            let since = *g.entry(id).or_insert(now);
            now.duration_since(since) >= SLOW_AFTER
        } else {
            g.remove(&id);
            false
        }
    }

    pub(crate) fn forget_speed(&self, id: TorrentId) {
        self.slow_since.lock().remove(&id);
    }
}

fn write_atomic(path: &Path, order: &[Id20]) -> anyhow::Result<()> {
    let f = QueueFile {
        order: order.iter().map(|h| h.as_string()).collect(),
    };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&f)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: usize, seeding: bool, active: bool, slow: bool) -> QueueCandidate {
        QueueCandidate {
            id,
            seeding,
            active,
            slow,
        }
    }

    fn sorted(s: HashSet<usize>) -> Vec<usize> {
        let mut v: Vec<_> = s.into_iter().collect();
        v.sort();
        v
    }

    #[test]
    fn unlimited_admits_everything() {
        let cands = vec![c(1, false, true, false), c(2, true, false, false), c(3, false, false, false)];
        assert_eq!(sorted(admit(&cands, &QueueLimits::default())), vec![1, 2, 3]);
    }

    #[test]
    fn download_and_upload_limits_follow_queue_order() {
        let l = QueueLimits {
            max_downloads: Some(2),
            max_uploads: Some(1),
            ..Default::default()
        };
        // Queue order: 5, 1, 4 (dl), 2, 3 (seed)
        let cands = vec![
            c(5, false, false, false),
            c(1, false, true, false),
            c(4, false, true, false),
            c(2, true, true, false),
            c(3, true, false, false),
        ];
        // 4 is running but lower in the queue than 5 -> held; 3 is over the upload limit.
        assert_eq!(sorted(admit(&cands, &l)), vec![1, 2, 5]);
    }

    #[test]
    fn total_limit_counts_both_kinds() {
        let l = QueueLimits {
            max_active: Some(2),
            ..Default::default()
        };
        let cands = vec![c(1, true, true, false), c(2, false, false, false), c(3, false, true, false)];
        assert_eq!(sorted(admit(&cands, &l)), vec![1, 2]);
    }

    #[test]
    fn slow_torrents_do_not_count_when_ignored() {
        let mut l = QueueLimits {
            max_downloads: Some(1),
            ..Default::default()
        };
        let cands = vec![c(1, false, true, true), c(2, false, false, false), c(3, false, false, false)];
        assert_eq!(sorted(admit(&cands, &l)), vec![1]);
        l.ignore_slow = true;
        // 1 keeps running (slow, not counted) and 2 gets the slot.
        assert_eq!(sorted(admit(&cands, &l)), vec![1, 2]);
        // Inactive torrents are never "slow".
        let cands = vec![c(1, false, false, true), c(2, false, false, false)];
        assert_eq!(sorted(admit(&cands, &l)), vec![1]);
    }

    #[test]
    fn zero_limit_holds_everything() {
        let l = QueueLimits {
            max_downloads: Some(0),
            ..Default::default()
        };
        let cands = vec![c(1, false, true, false), c(2, true, true, false)];
        assert_eq!(sorted(admit(&cands, &l)), vec![2]);
    }

    #[test]
    fn moves_preserve_relative_order() {
        let base = vec![1, 2, 3, 4, 5, 6];
        let sel: HashSet<i32> = [2, 3, 6].into_iter().collect();
        let mut v = base.clone();
        apply_move(&mut v, &sel, QueueMove::Up);
        assert_eq!(v, vec![2, 3, 1, 4, 6, 5]);
        let mut v = base.clone();
        apply_move(&mut v, &sel, QueueMove::Down);
        assert_eq!(v, vec![1, 4, 2, 3, 5, 6]);
        let mut v = base.clone();
        apply_move(&mut v, &sel, QueueMove::Top);
        assert_eq!(v, vec![2, 3, 6, 1, 4, 5]);
        let mut v = base.clone();
        apply_move(&mut v, &sel, QueueMove::Bottom);
        assert_eq!(v, vec![1, 4, 5, 2, 3, 6]);
        // Already at the top: Up is a no-op for that block.
        let mut v = vec![2, 3, 1];
        apply_move(&mut v, &[2, 3].into_iter().collect(), QueueMove::Up);
        assert_eq!(v, vec![2, 3, 1]);
    }

    #[test]
    fn sync_persist_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("queue.json");
        let h = |b: u8| Id20::new([b; 20]);
        let q = TorrentQueue::load(p.clone());
        q.sync(&[(2, h(2)), (1, h(1)), (3, h(3))]);
        assert_eq!(q.order(), vec![h(1), h(2), h(3)]);
        q.move_hashes(&[h(3)].into_iter().collect(), QueueMove::Top);
        assert_eq!(q.position(&h(3)), Some(1));
        // Removed torrent disappears, new one is appended.
        q.sync(&[(1, h(1)), (3, h(3)), (9, h(9))]);
        assert_eq!(q.order(), vec![h(3), h(1), h(9)]);
        let q2 = TorrentQueue::load(p);
        assert_eq!(q2.order(), vec![h(3), h(1), h(9)]);
    }

    #[test]
    fn slow_detection_needs_sustained_low_rate() {
        let q = TorrentQueue::default();
        let t0 = Instant::now();
        assert!(!q.observe_speed(1, 0, 0, t0));
        assert!(!q.observe_speed(1, 100, 0, t0 + Duration::from_secs(30)));
        assert!(q.observe_speed(1, 100, 0, t0 + SLOW_AFTER));
        assert!(!q.observe_speed(1, 10_000, 0, t0 + SLOW_AFTER + Duration::from_secs(1)));
    }
}
