//! Torrent list view state that doesn't need GPUI: status filter, search,
//! sorting and multi-selection. Mirrors the web UI's `torrentFilters.ts`,
//! `status.ts` (sort order) and `uiStore.ts` (selection).

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::api::{TorrentListItem, TorrentState, TorrentStats};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusFilter {
    All,
    Downloading,
    Seeding,
    Paused,
    Error,
    Stalled,
    Checking,
    Queued,
    Attention,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 9] = [
        StatusFilter::All,
        StatusFilter::Downloading,
        StatusFilter::Seeding,
        StatusFilter::Paused,
        StatusFilter::Error,
        StatusFilter::Stalled,
        StatusFilter::Checking,
        StatusFilter::Queued,
        StatusFilter::Attention,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "All",
            StatusFilter::Downloading => "Downloading",
            StatusFilter::Seeding => "Seeding",
            StatusFilter::Paused => "Paused",
            StatusFilter::Error => "Error",
            StatusFilter::Stalled => "Stalled",
            StatusFilter::Checking => "Checking",
            StatusFilter::Queued => "Queued",
            StatusFilter::Attention => "Needs attention / retrying",
        }
    }

    pub fn matches(self, t: &TorrentListItem) -> bool {
        let Some(s) = t.stats.as_ref() else {
            return self == StatusFilter::All;
        };
        let kind = s
            .status_detail
            .as_ref()
            .map(|d| d.kind.as_str())
            .unwrap_or("");
        match self {
            StatusFilter::All => true,
            StatusFilter::Downloading => s.state == TorrentState::Live && !s.finished,
            StatusFilter::Seeding => s.state == TorrentState::Live && s.finished,
            StatusFilter::Paused => s.state == TorrentState::Paused,
            StatusFilter::Error => s.state == TorrentState::Error,
            StatusFilter::Stalled => kind == "stalled",
            StatusFilter::Checking => kind == "checking" || kind == "queued_for_checking",
            StatusFilter::Queued => matches!(
                kind,
                "queued_for_downloading"
                    | "queued_for_seeding"
                    | "queued_for_checking"
                    | "queued_for_repair"
            ),
            StatusFilter::Attention => {
                matches!(
                    kind,
                    "needs_attention" | "waiting_to_retry" | "repairing" | "queued_for_repair"
                ) || s
                    .damage
                    .as_ref()
                    .is_some_and(|d| d.needs_attention == Some(true))
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortColumn {
    Id,
    Queue,
    Name,
    Status,
    Size,
    Progress,
    Down,
    Up,
    Received,
    Sent,
    Eta,
    Peers,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortDir {
    Asc,
    Desc,
}

/// Groups similar states together (web UI `STATUS_ORDER`).
const STATUS_ORDER: [&str; 18] = [
    "error",
    "needs_attention",
    "repairing",
    "queued_for_repair",
    "waiting_to_retry",
    "moving",
    "renaming",
    "checking",
    "queued_for_checking",
    "resolving_metadata",
    "initializing",
    "downloading",
    "stalled",
    "queued_for_downloading",
    "seeding",
    "queued_for_seeding",
    "complete",
    "paused",
];

fn status_rank(s: Option<&TorrentStats>) -> usize {
    match s {
        None => 999,
        Some(s) => STATUS_ORDER
            .iter()
            .position(|k| *k == s.status_kind())
            .unwrap_or(999),
    }
}

/// Seconds until done; 0 when finished, infinity when unknown.
pub fn eta_secs(s: &TorrentStats) -> f64 {
    let Some(live) = s.live.as_ref() else {
        return f64::INFINITY;
    };
    let remaining = s.total_bytes.saturating_sub(s.progress_bytes) as f64;
    let speed = live.download_speed.mbps;
    if remaining <= 0.0 {
        return 0.0;
    }
    if speed <= 0.0 {
        return f64::INFINITY;
    }
    remaining / (speed * 1024.0 * 1024.0)
}

enum Key {
    Num(f64),
    Str(String),
}

fn sort_key(t: &TorrentListItem, col: SortColumn) -> Key {
    let s = t.stats.as_ref();
    let live = s.and_then(|s| s.live.as_ref());
    Key::Num(match col {
        SortColumn::Id => t.id as f64,
        SortColumn::Name => return Key::Str(t.name.clone().unwrap_or_default().to_lowercase()),
        SortColumn::Size => s.map(|s| s.total_bytes as f64).unwrap_or(0.0),
        SortColumn::Progress => s
            .filter(|s| s.total_bytes > 0)
            .map(|s| s.progress_bytes as f64 / s.total_bytes as f64)
            .unwrap_or(0.0),
        SortColumn::Received => s.map(|s| s.progress_bytes as f64).unwrap_or(0.0),
        SortColumn::Down => live.map(|l| l.download_speed.mbps).unwrap_or(0.0),
        SortColumn::Up => live.map(|l| l.upload_speed.mbps).unwrap_or(0.0),
        SortColumn::Sent => live
            .map(|l| l.snapshot.uploaded_bytes as f64)
            .unwrap_or(0.0),
        SortColumn::Eta => s.map(eta_secs).unwrap_or(f64::INFINITY),
        SortColumn::Peers => live
            .map(|l| l.snapshot.peer_stats.live as f64)
            .unwrap_or(0.0),
        SortColumn::Queue => s
            .and_then(|s| s.queue_position)
            .map(|q| q as f64)
            .unwrap_or(f64::INFINITY),
        SortColumn::Status => status_rank(s) as f64,
    })
}

fn cmp_keys(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::Num(a), Key::Num(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (Key::Str(a), Key::Str(b)) => a.cmp(b),
        _ => Ordering::Equal,
    }
}

/// Indices into `torrents` of the visible rows, in display order.
pub fn visible_rows(
    torrents: &[TorrentListItem],
    query: &str,
    filter: StatusFilter,
    col: SortColumn,
    dir: SortDir,
) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut rows: Vec<(usize, Key)> = torrents
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            (q.is_empty() || t.name.as_deref().unwrap_or("").to_lowercase().contains(&q))
                && filter.matches(t)
        })
        .map(|(i, t)| (i, sort_key(t, col)))
        .collect();
    rows.sort_by(|(ia, a), (ib, b)| {
        let c = cmp_keys(a, b);
        let c = if dir == SortDir::Asc { c } else { c.reverse() };
        // Stable tiebreak by id so rows don't jump around between polls.
        c.then_with(|| torrents[*ia].id.cmp(&torrents[*ib].id))
    });
    rows.into_iter().map(|(i, _)| i).collect()
}

/// Multi-selection (web UI `uiStore.ts`).
#[derive(Default, Debug)]
pub struct Selection {
    pub ids: HashSet<usize>,
    anchor: Option<usize>,
}

impl Selection {
    pub fn contains(&self, id: usize) -> bool {
        self.ids.contains(&id)
    }
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn select_one(&mut self, id: usize) {
        self.ids = HashSet::from([id]);
        self.anchor = Some(id);
    }
    pub fn toggle(&mut self, id: usize) {
        if !self.ids.remove(&id) {
            self.ids.insert(id);
        }
        self.anchor = Some(id);
    }
    /// Shift-click: extend from the anchor to `id` in display order.
    pub fn select_range(&mut self, id: usize, ordered: &[usize]) {
        let Some(anchor) = self.anchor else {
            return self.select_one(id);
        };
        if self.ids.contains(&id) {
            self.ids.remove(&id);
            return;
        }
        let (Some(a), Some(b)) = (
            ordered.iter().position(|x| *x == anchor),
            ordered.iter().position(|x| *x == id),
        ) else {
            return self.select_one(id);
        };
        let (lo, hi) = (a.min(b), a.max(b));
        self.ids.extend(ordered[lo..=hi].iter().copied());
    }
    pub fn select_all(&mut self, ids: &[usize]) {
        self.ids = ids.iter().copied().collect();
    }
    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }
    /// Arrow keys: move a single selection up / down in display order.
    pub fn select_relative(&mut self, down: bool, ordered: &[usize]) {
        if ordered.is_empty() {
            return;
        }
        let current = if self.ids.len() == 1 {
            self.ids
                .iter()
                .next()
                .and_then(|id| ordered.iter().position(|x| x == id))
        } else if self.ids.is_empty() {
            let id = if down {
                ordered[0]
            } else {
                ordered[ordered.len() - 1]
            };
            return self.select_one(id);
        } else {
            self.anchor
                .and_then(|a| ordered.iter().position(|x| *x == a))
                .or_else(|| ordered.iter().position(|x| self.ids.contains(x)))
        };
        let Some(ix) = current else {
            return self.select_one(ordered[0]);
        };
        let next = if down {
            (ix + 1).min(ordered.len() - 1)
        } else {
            ix.saturating_sub(1)
        };
        self.select_one(ordered[next]);
    }
    /// Drop ids that no longer exist.
    pub fn retain_existing(&mut self, exists: impl Fn(usize) -> bool) {
        self.ids.retain(|id| exists(*id));
        if self.anchor.is_some_and(|a| !exists(a)) {
            self.anchor = None;
        }
    }
}

/// What "Fix errors" does for one torrent (web UI `fixErrorsSelected`).
#[derive(Debug, PartialEq, Eq)]
pub enum FixAction {
    /// Damaged files: repair them (also resumes).
    Repair,
    /// Error state or pieces held back: soft re-check.
    FixErrors,
    Skip,
}

pub fn fix_action(s: Option<&TorrentStats>) -> FixAction {
    let Some(s) = s else { return FixAction::Skip };
    let damaged = s.damage.as_ref().is_some_and(|d| d.has_damaged_files());
    if damaged && s.damage.as_ref().is_some_and(|d| d.is_repair_running()) {
        return FixAction::Skip;
    }
    let held = s.damage.as_ref().is_some_and(|d| d.has_recovery_issues());
    if damaged {
        FixAction::Repair
    } else if held || s.state == TorrentState::Error {
        FixAction::FixErrors
    } else {
        FixAction::Skip
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ListTorrentsResponse;

    fn list() -> Vec<TorrentListItem> {
        let v: ListTorrentsResponse = serde_json::from_value(serde_json::json!({"torrents": [
            {"id": 1, "info_hash": "a", "name": "Beta", "output_folder": "", "total_pieces": 1,
             "stats": {"state": "live", "finished": true, "progress_bytes": 10, "total_bytes": 10,
                       "status_detail": {"kind": "seeding", "label": "Seeding"}}},
            {"id": 2, "info_hash": "b", "name": "alpha", "output_folder": "", "total_pieces": 1,
             "stats": {"state": "error", "error": "boom", "progress_bytes": 5, "total_bytes": 50,
                       "status_detail": {"kind": "error", "label": "Error"}}},
            {"id": 3, "info_hash": "c", "name": "Gamma", "output_folder": "", "total_pieces": 1,
             "stats": {"state": "paused", "progress_bytes": 0, "total_bytes": 100, "queue_position": 1,
                       "status_detail": {"kind": "queued_for_downloading", "label": "Queued"},
                       "damage": {"damaged_files": [{"file_id": 0, "path": "x"}]}}}
        ]})).unwrap();
        v.torrents
    }

    #[test]
    fn filter_search_sort() {
        let t = list();
        let ids = |v: Vec<usize>| v.into_iter().map(|i| t[i].id).collect::<Vec<_>>();
        assert_eq!(
            ids(visible_rows(
                &t,
                "",
                StatusFilter::All,
                SortColumn::Id,
                SortDir::Desc
            )),
            [3, 2, 1]
        );
        assert_eq!(
            ids(visible_rows(
                &t,
                "",
                StatusFilter::All,
                SortColumn::Name,
                SortDir::Asc
            )),
            [2, 1, 3]
        );
        assert_eq!(
            ids(visible_rows(
                &t,
                "MM",
                StatusFilter::All,
                SortColumn::Id,
                SortDir::Asc
            )),
            [3]
        );
        assert_eq!(
            ids(visible_rows(
                &t,
                "",
                StatusFilter::Seeding,
                SortColumn::Id,
                SortDir::Asc
            )),
            [1]
        );
        assert_eq!(
            ids(visible_rows(
                &t,
                "",
                StatusFilter::Queued,
                SortColumn::Id,
                SortDir::Asc
            )),
            [3]
        );
        assert_eq!(
            ids(visible_rows(
                &t,
                "",
                StatusFilter::Error,
                SortColumn::Id,
                SortDir::Asc
            )),
            [2]
        );
        assert_eq!(
            ids(visible_rows(
                &t,
                "",
                StatusFilter::All,
                SortColumn::Status,
                SortDir::Asc
            )),
            [2, 3, 1]
        );
        assert_eq!(
            ids(visible_rows(
                &t,
                "",
                StatusFilter::All,
                SortColumn::Progress,
                SortDir::Desc
            )),
            [1, 2, 3]
        );
        // Queue: torrents without a position sort last ascending.
        assert_eq!(
            ids(visible_rows(
                &t,
                "",
                StatusFilter::All,
                SortColumn::Queue,
                SortDir::Asc
            )),
            [3, 1, 2]
        );
    }

    #[test]
    fn selection() {
        let order = [5, 4, 3, 2, 1];
        let mut s = Selection::default();
        s.select_one(4);
        s.select_range(2, &order);
        assert_eq!(s.len(), 3);
        s.toggle(3);
        assert!(!s.contains(3) && s.contains(4) && s.contains(2));
        s.select_relative(true, &order);
        assert_eq!(s.ids, HashSet::from([2]));
        s.select_relative(false, &order);
        assert_eq!(s.ids, HashSet::from([3]));
        s.clear();
        s.select_relative(true, &order);
        assert_eq!(s.ids, HashSet::from([5]));
        s.select_all(&order);
        s.retain_existing(|id| id != 1);
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn fix_errors_choice() {
        let t = list();
        assert_eq!(fix_action(t[0].stats.as_ref()), FixAction::Skip);
        assert_eq!(fix_action(t[1].stats.as_ref()), FixAction::FixErrors);
        assert_eq!(fix_action(t[2].stats.as_ref()), FixAction::Repair);
    }
}
