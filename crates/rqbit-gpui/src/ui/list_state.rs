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

/// Keyboard navigation target for [`Selection::navigate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Home,
    End,
}

/// File-manager style multi-selection over the displayed (sorted/filtered)
/// list, keyed by torrent id so it survives polling. Same semantics as the
/// web UI's `helper/selection.ts`:
///
/// - click: only that row; anchor + focus move there;
/// - shift+click: anchor..row replaces the selection (anchor kept);
///   ctrl+shift+click adds the range;
/// - ctrl/cmd+click, Space: toggle the row; anchor + focus move there;
/// - arrows / Home / End (with or without ctrl): move the focus only;
///   with shift: anchor..focus replaces the selection.
#[derive(Default, Debug)]
pub struct Selection {
    pub ids: HashSet<usize>,
    anchor: Option<usize>,
    focus: Option<usize>,
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
    /// The keyboard cursor (focus ring).
    pub fn focus(&self) -> Option<usize> {
        self.focus
    }
    /// The focused row if it is displayed.
    pub fn visible_focus(&self, ordered: &[usize]) -> Option<usize> {
        self.focus.filter(|f| ordered.contains(f))
    }
    /// Plain click.
    pub fn select_one(&mut self, id: usize) {
        self.ids = HashSet::from([id]);
        self.anchor = Some(id);
        self.focus = Some(id);
    }
    /// Ctrl/Cmd+click, checkbox, Space.
    pub fn toggle(&mut self, id: usize) {
        if !self.ids.remove(&id) {
            self.ids.insert(id);
        }
        self.anchor = Some(id);
        self.focus = Some(id);
    }
    /// Shift+click (`additive` = ctrl+shift+click): select anchor..`id` in
    /// display order. The anchor stays put so repeated shift-clicks re-span
    /// from the same row.
    pub fn select_range(&mut self, id: usize, ordered: &[usize], additive: bool) {
        let anchor_ix = self
            .anchor
            .and_then(|a| ordered.iter().position(|x| *x == a));
        let (Some(a), Some(b)) = (anchor_ix, ordered.iter().position(|x| *x == id)) else {
            if additive {
                return self.toggle(id);
            }
            return self.select_one(id);
        };
        let (lo, hi) = (a.min(b), a.max(b));
        if !additive {
            self.ids.clear();
        }
        self.ids.extend(ordered[lo..=hi].iter().copied());
        self.focus = Some(id);
    }
    /// Arrow keys / Home / End. Moves the focus; with `extend` (shift) the
    /// selection becomes anchor..focus. Returns the new focus' index in
    /// `ordered` (to scroll it into view).
    pub fn navigate(&mut self, nav: Nav, extend: bool, ordered: &[usize]) -> Option<usize> {
        if ordered.is_empty() {
            return None;
        }
        let last = ordered.len() - 1;
        let pos = |id: usize| ordered.iter().position(|x| *x == id);
        // Where the cursor is: the focus, else the first displayed selected row.
        let current = self
            .focus
            .and_then(pos)
            .or_else(|| ordered.iter().position(|x| self.ids.contains(x)));
        let target = match (nav, current) {
            (Nav::Home, _) => 0,
            (Nav::End, _) => last,
            (Nav::Up, Some(i)) => i.saturating_sub(1),
            (Nav::Down, Some(i)) => (i + 1).min(last),
            (Nav::Up, None) => last,
            (Nav::Down, None) => 0,
        };
        if extend {
            let anchor = self
                .anchor
                .filter(|a| pos(*a).is_some())
                .or(current.map(|i| ordered[i]))
                .unwrap_or(ordered[target]);
            let a = pos(anchor).unwrap_or(target);
            let (lo, hi) = (a.min(target), a.max(target));
            self.ids = ordered[lo..=hi].iter().copied().collect();
            self.anchor = Some(anchor);
        }
        self.focus = Some(ordered[target]);
        Some(target)
    }
    /// Space: toggle the focused (displayed) row.
    pub fn toggle_focused(&mut self, ordered: &[usize]) -> bool {
        match self.visible_focus(ordered) {
            Some(f) => {
                self.toggle(f);
                true
            }
            None => false,
        }
    }
    /// Ctrl+A / header checkbox.
    pub fn select_all(&mut self, ids: &[usize]) {
        self.ids = ids.iter().copied().collect();
    }
    /// Esc. The focus stays so the keyboard cursor doesn't jump.
    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }
    /// Drop ids that no longer exist (after a list refresh).
    pub fn retain_existing(&mut self, exists: impl Fn(usize) -> bool) {
        self.ids.retain(|id| exists(*id));
        if self.anchor.is_some_and(|a| !exists(a)) {
            self.anchor = None;
        }
        if self.focus.is_some_and(|f| !exists(f)) {
            self.focus = None;
        }
    }
}

/// How Remove/Delete behaves given the server preferences
/// (`confirm_remove`, `default_remove_action`; web UI `helper/removePrefs.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovePlan {
    /// Show the confirmation dialog.
    pub confirm: bool,
    /// Preset of "also delete files" (the action when there is no dialog).
    pub delete_files: bool,
}

/// Deleting files never happens without a confirmation, so a "delete files"
/// default always shows the dialog; unknown preferences also confirm.
pub fn plan_remove(prefs: Option<&serde_json::Map<String, serde_json::Value>>) -> RemovePlan {
    let Some(p) = prefs else {
        return RemovePlan {
            confirm: true,
            delete_files: false,
        };
    };
    let delete_files =
        p.get("default_remove_action").and_then(|v| v.as_str()) == Some("delete_files");
    let confirm = p.get("confirm_remove").and_then(|v| v.as_bool()) != Some(false) || delete_files;
    RemovePlan {
        confirm,
        delete_files,
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

    fn set(v: &[usize]) -> HashSet<usize> {
        v.iter().copied().collect()
    }

    #[test]
    fn selection_clicks() {
        let order = [5, 4, 3, 2, 1];
        let mut s = Selection::default();
        s.select_one(4);
        assert_eq!((s.ids.clone(), s.focus()), (set(&[4]), Some(4)));
        // Shift+click spans from the anchor and replaces the selection.
        s.select_range(2, &order, false);
        assert_eq!(s.ids, set(&[4, 3, 2]));
        assert_eq!(s.focus(), Some(2));
        // Anchor unchanged: shift+click the other way shrinks/re-spans.
        s.select_range(5, &order, false);
        assert_eq!(s.ids, set(&[5, 4]));
        // Ctrl+click toggles and moves the anchor.
        s.toggle(1);
        assert_eq!(s.ids, set(&[5, 4, 1]));
        s.toggle(4);
        assert_eq!(s.ids, set(&[5, 1]));
        // Ctrl+shift+click adds anchor(4)..2 to the existing selection.
        s.select_range(2, &order, true);
        assert_eq!(s.ids, set(&[5, 4, 3, 2, 1]));
        // Shift+click without an anchor behaves like a click.
        s.clear();
        s.select_range(3, &order, false);
        assert_eq!(s.ids, set(&[3]));
        // Anchor filtered out of the display: plain click.
        s.select_range(1, &[1, 2], false);
        assert_eq!(s.ids, set(&[1]));
    }

    #[test]
    fn selection_keyboard() {
        let order = [5, 4, 3, 2, 1];
        let mut s = Selection::default();
        // Arrows move the cursor only.
        assert_eq!(s.navigate(Nav::Down, false, &order), Some(0));
        assert_eq!(s.focus(), Some(5));
        assert!(s.is_empty());
        s.navigate(Nav::Down, false, &order);
        assert_eq!(s.focus(), Some(4));
        assert!(s.is_empty());
        // Space toggles the focused row and anchors there.
        assert!(s.toggle_focused(&order));
        assert_eq!(s.ids, set(&[4]));
        // Shift+Down extends anchor..focus, shift+Up shrinks it.
        s.navigate(Nav::Down, true, &order);
        s.navigate(Nav::Down, true, &order);
        assert_eq!(s.ids, set(&[4, 3, 2]));
        s.navigate(Nav::Up, true, &order);
        assert_eq!(s.ids, set(&[4, 3]));
        // Past the anchor flips direction.
        s.navigate(Nav::Up, true, &order);
        s.navigate(Nav::Up, true, &order);
        assert_eq!(s.ids, set(&[5, 4]));
        assert_eq!(s.focus(), Some(5));
        // Clamped at the top.
        assert_eq!(s.navigate(Nav::Up, false, &order), Some(0));
        // Ctrl+Down / plain Down: cursor moves, selection kept; Space adds.
        s.navigate(Nav::Down, false, &order);
        s.navigate(Nav::Down, false, &order);
        s.navigate(Nav::Down, false, &order);
        assert_eq!(s.focus(), Some(2));
        s.toggle_focused(&order);
        assert_eq!(s.ids, set(&[5, 4, 2]));
        s.toggle_focused(&order);
        assert_eq!(s.ids, set(&[5, 4]));
        // Shift+End / Shift+Home from the anchor (2).
        assert_eq!(s.navigate(Nav::End, true, &order), Some(4));
        assert_eq!(s.ids, set(&[2, 1]));
        s.navigate(Nav::Home, true, &order);
        assert_eq!(s.ids, set(&[5, 4, 3, 2]));
        s.navigate(Nav::End, false, &order);
        assert_eq!(s.focus(), Some(1));
        // Select all / clear keeps the cursor.
        s.select_all(&order);
        assert_eq!(s.len(), 5);
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.focus(), Some(1));
        // Up with no cursor starts at the bottom; Space needs a visible cursor.
        let mut t = Selection::default();
        assert!(!t.toggle_focused(&order));
        t.navigate(Nav::Up, false, &order);
        assert_eq!(t.focus(), Some(1));
        assert!(!t.toggle_focused(&[5, 4]));
        assert_eq!(t.navigate(Nav::Down, false, &[]), None);
    }

    #[test]
    fn selection_survives_refresh() {
        let mut s = Selection::default();
        s.select_one(3);
        s.navigate(Nav::Down, true, &[3, 2, 1]);
        s.retain_existing(|id| id != 2);
        assert_eq!(s.ids, set(&[3]));
        assert_eq!(s.focus(), None);
        // Anchor 3 still exists: shift+click spans from it in the new order.
        s.select_range(1, &[3, 1], false);
        assert_eq!(s.ids, set(&[3, 1]));
    }

    #[test]
    fn remove_plan() {
        let plan = |v: serde_json::Value| plan_remove(v.as_object());
        let p = |confirm, delete_files| RemovePlan {
            confirm,
            delete_files,
        };
        assert_eq!(plan_remove(None), p(true, false));
        assert_eq!(plan(serde_json::json!({})), p(true, false));
        assert_eq!(
            plan(serde_json::json!({"confirm_remove": false})),
            p(false, false)
        );
        assert_eq!(
            plan(serde_json::json!({"default_remove_action": "delete_files"})),
            p(true, true)
        );
        // Deleting files always asks.
        assert_eq!(
            plan(
                serde_json::json!({"confirm_remove": false, "default_remove_action": "delete_files"})
            ),
            p(true, true)
        );
    }

    #[test]
    fn fix_errors_choice() {
        let t = list();
        assert_eq!(fix_action(t[0].stats.as_ref()), FixAction::Skip);
        assert_eq!(fix_action(t[1].stats.as_ref()), FixAction::FixErrors);
        assert_eq!(fix_action(t[2].stats.as_ref()), FixAction::Repair);
    }
}
