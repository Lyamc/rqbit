//! GPUI views.
//!
//! `RqbitWindow` is the root view: connection bar, status/error banners, the
//! torrent table and a confirm dialog. New web-UI features should be added as
//! their own modules/views here (e.g. `details.rs`, `add_torrent.rs`,
//! `events.rs`) with any new endpoints going into `crate::api`.

mod add_panel;
mod details;
mod events_panel;
mod files;
mod list_state;
mod prefs_panel;
mod server_browser;
mod text_input;
mod theme;
mod torrent_table;
mod widgets;

use std::collections::HashSet;
use std::time::Duration;

use crate::time::Instant;

use gpui::{
    ClickEvent, Context, Entity, FocusHandle, KeyDownEvent, ScrollStrategy, SharedString,
    Subscription, Task, UniformListScrollHandle, Window, deferred, div, prelude::*, px,
    uniform_list,
};

use crate::api::{
    self, ApiClient, ListTorrentsResponse, TorrentListItem, Transport, parse_base_url,
};
use crate::format::{format_bytes, format_speed, format_uptime};
use add_panel::{AddPanel, AddPanelEvent};
use details::{DetailsEvent, DetailsPanel};
use events_panel::{EventsPanel, EventsPanelEvent};
use list_state::{FixAction, Nav, Selection, SortColumn, SortDir, StatusFilter};
use prefs_panel::{PrefsPanel, PrefsPanelEvent};
use text_input::{TextInput, TextInputEvent};

/// How often the torrent list is refreshed (the web UI polls about as often).
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Slower retry while the server is unreachable.
const RETRY_INTERVAL: Duration = Duration::from_secs(3);
/// Events badge refresh (web UI: 15 s).
/// Footer session stats refresh.
const STATS_INTERVAL: Duration = Duration::from_secs(2);
const EVENTS_SUMMARY_INTERVAL: Duration = Duration::from_secs(15);

/// Per-torrent actions, run one torrent at a time over a set of ids
/// (the web UI's action bar does the same).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TorrentAction {
    Start,
    Pause,
    Restart,
    FixErrors,
    Recheck,
    Forget,
    Delete,
}

impl TorrentAction {
    fn verb(self) -> &'static str {
        match self {
            TorrentAction::Start => "Resume",
            TorrentAction::Pause => "Pause",
            TorrentAction::Restart => "Restart",
            TorrentAction::FixErrors => "Fix errors",
            TorrentAction::Recheck => "Force recheck",
            TorrentAction::Forget => "Remove",
            TorrentAction::Delete => "Delete",
        }
    }
}

struct DeleteDialog {
    items: Vec<(usize, SharedString)>,
    delete_files: bool,
}

enum ConnState {
    /// The URL typed in the connection field can't be used.
    Invalid(String),
    Connecting,
    Connected,
    Unreachable {
        error: String,
        since: Instant,
    },
}

pub struct RqbitWindow {
    transport: Transport,
    client: Option<ApiClient>,
    url_input: Entity<TextInput>,
    conn: ConnState,
    /// Bumped on every (re)connect; late results from an old connection are dropped.
    generation: u64,
    torrents: Vec<TorrentListItem>,
    last_update: Option<Instant>,
    /// Torrent ids with an in-flight start/pause/remove.
    pending: HashSet<usize>,
    action_error: Option<String>,
    confirm_delete: Option<DeleteDialog>,
    selection: Selection,
    /// Scroll position of the torrent list (keeps the keyboard cursor in view).
    list_scroll: UniformListScrollHandle,
    filter: StatusFilter,
    filter_menu_open: bool,
    sort: SortColumn,
    sort_dir: SortDir,
    search_input: Entity<TextInput>,
    /// Indices into `torrents` of the visible rows, in display order
    /// (recomputed every render).
    visible: Vec<usize>,
    /// A bulk action is running.
    bulk_busy: bool,
    focus_handle: FocusHandle,
    poll_task: Option<Task<()>>,
    add_panel: Option<(Entity<AddPanel>, Subscription)>,
    details: Option<(Entity<DetailsPanel>, Subscription)>,
    /// The details pane was closed; reopened by double click / Enter.
    details_hidden: bool,
    prefs_panel: Option<(Entity<PrefsPanel>, Subscription)>,
    events_panel: Option<(Entity<EventsPanel>, Subscription)>,
    /// Latest `/events/summary?since_seq=<last seen>` (header badge).
    events_summary: Option<api::EventSummary>,
    /// Highest event seq the user has seen (persisted per server).
    events_seen: Option<u64>,
    events_task: Option<Task<()>>,
    /// Footer: `/stats` and `/torrents/limits`.
    session_stats: Option<api::SessionStats>,
    limits: Option<api::LimitsConfig>,
    public_ip: Option<api::PublicIp>,
    /// Server preferences used by the UI itself (remove/delete behaviour).
    ui_prefs: Option<serde_json::Map<String, serde_json::Value>>,
    stats_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RqbitWindow {
    pub fn new(
        transport: Transport,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url_input = cx.new(|cx| TextInput::new(url.clone(), "http://host:3030", cx));
        let sub = cx.subscribe_in(
            &url_input,
            window,
            |this, _, ev: &TextInputEvent, _, cx| match ev {
                TextInputEvent::Submit(url) => this.connect(url.clone(), cx),
                TextInputEvent::Changed => {}
            },
        );
        let search_input = cx.new(|cx| TextInput::new("", "Search…", cx));
        let search_sub = cx.subscribe(&search_input, |_, _, _: &TextInputEvent, cx| cx.notify());
        let mut this = Self {
            transport,
            client: None,
            url_input,
            conn: ConnState::Connecting,
            generation: 0,
            torrents: Vec::new(),
            last_update: None,
            pending: HashSet::new(),
            action_error: None,
            confirm_delete: None,
            selection: Selection::default(),
            list_scroll: UniformListScrollHandle::new(),
            filter: StatusFilter::All,
            filter_menu_open: false,
            sort: SortColumn::Id,
            sort_dir: SortDir::Desc,
            search_input,
            visible: Vec::new(),
            bulk_busy: false,
            focus_handle: cx.focus_handle(),
            poll_task: None,
            add_panel: None,
            details: None,
            details_hidden: false,
            prefs_panel: None,
            events_panel: None,
            events_summary: None,
            events_seen: None,
            events_task: None,
            session_stats: None,
            limits: None,
            public_ip: None,
            ui_prefs: None,
            stats_task: None,
            _subscriptions: vec![sub, search_sub],
        };
        this.connect(url, cx);
        // Browser: files chosen in the file input or dropped on the page.
        #[cfg(target_family = "wasm")]
        {
            let mut rx = files::web_files_channel();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                while let Some(files) = rx.next().await {
                    if this
                        .update(cx, |this, cx| {
                            if let Some(p) = this.open_add(cx) {
                                p.update(cx, |p, cx| p.add_files(files, "file", cx));
                            }
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            })
            .detach();
        }
        this
    }

    fn connect(&mut self, raw: String, cx: &mut Context<Self>) {
        self.generation += 1;
        self.torrents.clear();
        self.pending.clear();
        self.last_update = None;
        self.action_error = None;
        self.confirm_delete = None;
        self.selection.clear();
        self.details = None;
        self.poll_task = None;
        self.client = None;
        self.events_panel = None;
        self.events_summary = None;
        self.events_task = None;
        self.stats_task = None;
        self.session_stats = None;
        self.limits = None;
        self.public_ip = None;
        self.ui_prefs = None;

        match parse_base_url(&raw) {
            Err(e) => self.conn = ConnState::Invalid(format!("{e:#}")),
            Ok(url) => {
                let client = ApiClient::new(self.transport.clone(), url);
                self.client = Some(client.clone());
                self.conn = ConnState::Connecting;
                let generation = self.generation;
                let client_for_stats = client.clone();
                self.poll_task = Some(cx.spawn(async move |this, cx| {
                    loop {
                        let res = cx.background_executor().spawn(client.list_torrents()).await;
                        let ok = res.is_ok();
                        let alive = this
                            .update(cx, |this, cx| {
                                if this.generation == generation {
                                    this.apply_list(res, cx);
                                    cx.notify();
                                }
                            })
                            .is_ok();
                        if !alive {
                            return;
                        }
                        let delay = if ok { POLL_INTERVAL } else { RETRY_INTERVAL };
                        cx.background_executor().timer(delay).await;
                    }
                }));
                self.events_seen = crate::store::get(&self.seen_key()).and_then(|v| v.parse().ok());
                let stats_client = client_for_stats.clone();
                self.stats_task = Some(cx.spawn(async move |this, cx| {
                    let mut n: u64 = 0;
                    loop {
                        let stats = cx
                            .background_executor()
                            .spawn(stats_client.session_stats())
                            .await;
                        // Public IP: every 60 s (the server re-checks every 5 min).
                        let public_ip = if n.is_multiple_of(30) {
                            Some(
                                cx.background_executor()
                                    .spawn(stats_client.public_ip(false))
                                    .await,
                            )
                        } else {
                            None
                        };
                        // Remove/delete preferences (may be changed elsewhere).
                        let prefs = if n.is_multiple_of(15) {
                            Some(
                                cx.background_executor()
                                    .spawn(stats_client.get_preferences())
                                    .await,
                            )
                        } else {
                            None
                        };
                        let limits = if n.is_multiple_of(5) {
                            Some(
                                cx.background_executor()
                                    .spawn(stats_client.get_limits())
                                    .await,
                            )
                        } else {
                            None
                        };
                        let alive = this
                            .update(cx, |this, cx| {
                                if this.generation != generation {
                                    return;
                                }
                                if let Ok(s) = stats {
                                    this.session_stats = Some(s);
                                }
                                if let Some(Ok(l)) = limits {
                                    this.limits = Some(l);
                                }
                                if let Some(Ok(p)) = public_ip {
                                    this.public_ip = Some(p);
                                }
                                if let Some(Ok(p)) = prefs {
                                    this.ui_prefs = Some(p);
                                }
                                cx.notify();
                            })
                            .is_ok();
                        if !alive {
                            return;
                        }
                        n += 1;
                        cx.background_executor().timer(STATS_INTERVAL).await;
                    }
                }));
                self.events_task = Some(cx.spawn(async move |this, cx| {
                    loop {
                        let Ok(()) = this.update(cx, |this, cx| this.refresh_events_summary(cx))
                        else {
                            return;
                        };
                        cx.background_executor()
                            .timer(EVENTS_SUMMARY_INTERVAL)
                            .await;
                    }
                }));
            }
        }
        cx.notify();
    }

    fn apply_list(&mut self, res: anyhow::Result<ListTorrentsResponse>, cx: &mut Context<Self>) {
        match res {
            Ok(list) => {
                self.torrents = list.torrents;
                let torrents = &self.torrents;
                self.selection
                    .retain_existing(|id| torrents.iter().any(|t| t.id == id));
                self.last_update = Some(Instant::now());
                self.conn = ConnState::Connected;
                if let Some((p, _)) = &self.events_panel {
                    let known = self.known_hashes();
                    p.update(cx, |p, cx| {
                        p.set_known(known);
                        cx.notify();
                    });
                }
            }
            Err(e) => {
                let since = match &self.conn {
                    ConnState::Unreachable { since, .. } => *since,
                    _ => Instant::now(),
                };
                self.conn = ConnState::Unreachable {
                    error: format!("{e:#}"),
                    since,
                };
            }
        }
    }

    /// Runs `action` on each id in turn. Like the web UI, torrents already in
    /// the target state are skipped, errors are collected, and (for the action
    /// bar) the selection is cleared afterwards.
    pub(crate) fn run_action(
        &mut self,
        ids: Vec<usize>,
        action: TorrentAction,
        clear_selection: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let jobs: Vec<(usize, TorrentAction, FixAction)> = ids
            .into_iter()
            .filter(|id| !self.pending.contains(id))
            .filter_map(|id| {
                let stats = self
                    .torrents
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.stats.as_ref());
                let state = stats.map(|s| s.state);
                let skip = match action {
                    TorrentAction::Start => state == Some(api::TorrentState::Live),
                    TorrentAction::Pause => state == Some(api::TorrentState::Paused),
                    TorrentAction::FixErrors => list_state::fix_action(stats) == FixAction::Skip,
                    _ => false,
                };
                (!skip).then(|| (id, action, list_state::fix_action(stats)))
            })
            .collect();
        if clear_selection {
            self.selection.clear();
        }
        if jobs.is_empty() {
            cx.notify();
            return;
        }
        for (id, _, _) in &jobs {
            self.pending.insert(*id);
        }
        self.bulk_busy = true;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let mut errors = Vec::new();
            for (id, action, fix) in jobs {
                let request = match action {
                    TorrentAction::Start => client.start(id),
                    TorrentAction::Pause => client.pause(id),
                    TorrentAction::Restart => client.restart(id),
                    TorrentAction::FixErrors if fix == FixAction::Repair => {
                        client.repair_files(id, None)
                    }
                    TorrentAction::FixErrors => client.fix_errors(id),
                    TorrentAction::Recheck => client.recheck(id),
                    TorrentAction::Forget => client.forget(id),
                    TorrentAction::Delete => client.delete(id),
                };
                let res = cx.background_executor().spawn(request).await;
                if let Err(e) = res {
                    errors.push(format!("#{id}: {e:#}"));
                }
                let list = cx.background_executor().spawn(client.list_torrents()).await;
                let alive = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        this.pending.remove(&id);
                        this.apply_list(list, cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.bulk_busy = false;
                if !errors.is_empty() {
                    this.action_error = Some(format!(
                        "{} failed for {} torrent{}: {}",
                        action.verb(),
                        errors.len(),
                        if errors.len() == 1 { "" } else { "s" },
                        errors.join("; ")
                    ));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn run_on_selection(&mut self, action: TorrentAction, cx: &mut Context<Self>) {
        let ids = self.selected_in_order();
        self.run_action(ids, action, true, cx);
    }

    /// Selected ids in display order (hidden-by-filter selections last).
    fn selected_in_order(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = self
            .visible
            .iter()
            .map(|ix| self.torrents[*ix].id)
            .filter(|id| self.selection.contains(*id))
            .collect();
        let mut rest: Vec<usize> = self
            .selection
            .ids
            .iter()
            .copied()
            .filter(|id| !ids.contains(id))
            .collect();
        rest.sort_unstable();
        ids.extend(rest);
        ids
    }

    fn queue_move(&mut self, action: &'static str, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        // Server keeps the relative order of the ids given.
        let mut ids: Vec<usize> = self.selection.ids.iter().copied().collect();
        ids.sort_by_key(|id| {
            self.torrents
                .iter()
                .find(|t| t.id == *id)
                .and_then(|t| t.stats.as_ref()?.queue_position)
                .unwrap_or(u32::MAX)
        });
        if ids.is_empty() {
            return;
        }
        self.bulk_busy = true;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(client.queue_move(&ids, action))
                .await;
            let list = cx.background_executor().spawn(client.list_torrents()).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.bulk_busy = false;
                if let Err(e) = res {
                    this.action_error = Some(format!("Error moving torrents in queue: {e:#}"));
                }
                this.apply_list(list, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn ask_delete(&mut self, cx: &mut Context<Self>) {
        let items: Vec<(usize, SharedString)> = self
            .selected_in_order()
            .into_iter()
            .map(|id| {
                let name = self
                    .torrents
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.display_name())
                    .unwrap_or_else(|| format!("#{id}"));
                (id, name.into())
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let plan = list_state::plan_remove(self.ui_prefs.as_ref());
        if !plan.confirm && !plan.delete_files {
            // Confirmation off in Preferences: remove now, keep the files.
            let ids = items.into_iter().map(|(id, _)| id).collect();
            self.run_action(ids, TorrentAction::Forget, true, cx);
            return;
        }
        self.confirm_delete = Some(DeleteDialog {
            items,
            delete_files: plan.delete_files,
        });
        cx.notify();
    }

    pub(crate) fn sort_by(&mut self, col: SortColumn, cx: &mut Context<Self>) {
        if self.sort == col {
            self.sort_dir = match self.sort_dir {
                SortDir::Asc => SortDir::Desc,
                SortDir::Desc => SortDir::Asc,
            };
        } else {
            self.sort = col;
            self.sort_dir = SortDir::Desc;
        }
        cx.notify();
    }

    fn visible_ids(&self) -> Vec<usize> {
        self.visible
            .iter()
            .map(|ix| self.torrents[*ix].id)
            .collect()
    }

    pub(crate) fn toggle_select_all(&mut self, cx: &mut Context<Self>) {
        let ids = self.visible_ids();
        if !ids.is_empty() && ids.iter().all(|id| self.selection.contains(*id)) {
            self.selection.clear();
        } else {
            self.selection.select_all(&ids);
        }
        cx.notify();
    }

    pub(crate) fn toggle_selected(
        &mut self,
        id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        text_input::focus(window, &self.focus_handle, cx);
        self.selection.toggle(id);
        cx.notify();
    }

    pub(crate) fn row_clicked(
        &mut self,
        id: usize,
        ev: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        text_input::focus(window, &self.focus_handle, cx);
        let m = ev.modifiers();
        let cmd = m.control || m.platform;
        if m.shift {
            let ids = self.visible_ids();
            self.selection.select_range(id, &ids, cmd);
        } else if cmd {
            self.selection.toggle(id);
        } else {
            self.selection.select_one(id);
            if ev.click_count() >= 2 {
                self.open_details(id, cx);
            }
        }
        cx.notify();
    }

    fn open_details(&mut self, id: usize, cx: &mut Context<Self>) {
        self.details_hidden = false;
        self.selection.select_one(id);
        cx.notify();
    }

    /// Keeps the details pane in sync with the selection: shown for exactly
    /// one selected torrent unless the user closed it.
    fn sync_details(&mut self, cx: &mut Context<Self>) -> Option<Entity<DetailsPanel>> {
        let single = (self.selection.len() == 1)
            .then(|| self.selection.ids.iter().next().copied())
            .flatten();
        let (Some(id), Some(client)) =
            (single.filter(|_| !self.details_hidden), self.client.clone())
        else {
            self.details = None;
            return None;
        };
        if self
            .details
            .as_ref()
            .is_none_or(|(p, _)| p.read(cx).id() != id)
        {
            let panel = cx.new(|cx| DetailsPanel::new(client, id, cx));
            let sub = cx.subscribe(&panel, |this, _, ev: &DetailsEvent, cx| match ev {
                DetailsEvent::Action(id, a) => this.run_action(vec![*id], *a, false, cx),
                DetailsEvent::Close => {
                    this.details_hidden = true;
                    cx.notify();
                }
                DetailsEvent::Changed => this.refresh_now(cx),
                DetailsEvent::OpenEvents(hash, name) => {
                    this.open_events(Some((hash.clone(), name.clone())), cx)
                }
            });
            self.details = Some((panel, sub));
        }
        let panel = self.details.as_ref().map(|(p, _)| p.clone())?;
        let t = self.torrents.iter().find(|t| t.id == id).cloned();
        panel.update(cx, |p, _| p.set_torrent(t));
        Some(panel)
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Only when the list itself has focus (not a text field).
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let cmd = m.control || m.platform;
        let nav = match ks.key.as_str() {
            "up" => Some(Nav::Up),
            "down" => Some(Nav::Down),
            "home" => Some(Nav::Home),
            "end" => Some(Nav::End),
            _ => None,
        };
        if let Some(nav) = nav {
            // Plain / ctrl: move the cursor only; shift: extend the range.
            let ids = self.visible_ids();
            if let Some(ix) = self.selection.navigate(nav, m.shift, &ids) {
                self.reveal_row(ix, nav);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match ks.key.as_str() {
            "space" if !m.shift && !m.alt => {
                let ids = self.visible_ids();
                self.selection.toggle_focused(&ids);
            }
            "a" if cmd && !m.shift => {
                let ids = self.visible_ids();
                self.selection.select_all(&ids);
            }
            "delete" => self.ask_delete(cx),
            "escape" => {
                self.filter_menu_open = false;
                self.selection.clear();
            }
            "enter" => {
                // Details for the cursor row (or the single selected one).
                let ids = self.visible_ids();
                let target = self.selection.visible_focus(&ids).or_else(|| {
                    (self.selection.len() == 1)
                        .then(|| self.selection.ids.iter().next().copied())
                        .flatten()
                });
                if let Some(id) = target {
                    self.open_details(id, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Scrolls the list just enough to show row `ix` (at the edge it moved
    /// towards).
    fn reveal_row(&self, ix: usize, nav: Nav) {
        let strategy = match nav {
            Nav::Down | Nav::End => ScrollStrategy::Bottom,
            Nav::Up | Nav::Home => ScrollStrategy::Top,
        };
        self.list_scroll.scroll_to_item(ix, strategy);
    }

    /// Opens (or returns the open) Add panel.
    fn open_add(&mut self, cx: &mut Context<Self>) -> Option<Entity<AddPanel>> {
        if let Some((p, _)) = &self.add_panel {
            return Some(p.clone());
        }
        let client = self.client.clone()?;
        self.prefs_panel = None;
        self.events_panel = None;
        let panel = cx.new(|cx| AddPanel::new(client, cx));
        let sub = cx.subscribe(&panel, |this, _, ev: &AddPanelEvent, cx| match ev {
            AddPanelEvent::Close => {
                this.add_panel = None;
                cx.notify();
            }
            AddPanelEvent::Added => this.refresh_now(cx),
        });
        self.add_panel = Some((panel.clone(), sub));
        cx.notify();
        Some(panel)
    }

    fn seen_key(&self) -> String {
        format!(
            "events-last-seen-seq:{}",
            self.client
                .as_ref()
                .map(|c| c.base_url().to_string())
                .unwrap_or_default()
        )
    }

    fn known_hashes(&self) -> HashSet<String> {
        self.torrents.iter().map(|t| t.info_hash.clone()).collect()
    }

    fn mark_events_seen(&mut self, seq: u64, cx: &mut Context<Self>) {
        if self.events_seen.is_some_and(|s| s >= seq) {
            return;
        }
        self.events_seen = Some(seq);
        crate::store::set(&self.seen_key(), &seq.to_string());
        self.refresh_events_summary(cx);
    }

    /// Header badge: unseen repairs / errors since the last viewed seq. On
    /// first run everything up to now counts as seen (web UI behaviour).
    fn refresh_events_summary(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let generation = self.generation;
        let seen = self.events_seen;
        cx.spawn(async move |this, cx| {
            let mut seen = seen;
            if seen.is_none() {
                let Ok(s) = cx
                    .background_executor()
                    .spawn(client.get_events_summary(None))
                    .await
                else {
                    return;
                };
                let ok = this
                    .update(cx, |this, _| {
                        if this.generation == generation {
                            this.events_seen = Some(s.latest_seq);
                            crate::store::set(&this.seen_key(), &s.latest_seq.to_string());
                        }
                    })
                    .is_ok();
                if !ok {
                    return;
                }
                seen = Some(s.latest_seq);
            }
            let r = cx
                .background_executor()
                .spawn(client.get_events_summary(seen))
                .await;
            this.update(cx, |this, cx| {
                if this.generation == generation
                    && let Ok(s) = r
                {
                    this.events_summary = Some(s);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Opens the Events view, optionally filtered to one torrent.
    fn open_events(&mut self, torrent: Option<(String, String)>, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if self
            .add_panel
            .as_ref()
            .is_some_and(|(p, _)| p.read(cx).is_running())
        {
            return;
        }
        self.add_panel = None;
        self.prefs_panel = None;
        let known = self.known_hashes();
        let panel = cx.new(|cx| EventsPanel::new(client, torrent, known, cx));
        let sub = cx.subscribe(&panel, |this, _, ev: &EventsPanelEvent, cx| match ev {
            EventsPanelEvent::Close => {
                this.events_panel = None;
                cx.notify();
            }
            EventsPanelEvent::Seen(seq) => this.mark_events_seen(*seq, cx),
            EventsPanelEvent::OpenTorrent(hash) => {
                if let Some(id) = this
                    .torrents
                    .iter()
                    .find(|t| &t.info_hash == hash)
                    .map(|t| t.id)
                {
                    this.events_panel = None;
                    this.open_details(id, cx);
                }
            }
        });
        self.events_panel = Some((panel, sub));
        cx.notify();
    }

    fn open_prefs(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if self
            .add_panel
            .as_ref()
            .is_some_and(|(p, _)| p.read(cx).is_running())
        {
            return;
        }
        self.add_panel = None;
        self.events_panel = None;
        let panel = cx.new(|cx| PrefsPanel::new(client, cx));
        let sub = cx.subscribe(&panel, |this, _, ev: &PrefsPanelEvent, cx| match ev {
            PrefsPanelEvent::Close => {
                this.prefs_panel = None;
                this.reload_ui_prefs(cx);
                cx.notify();
            }
        });
        self.prefs_panel = Some((panel, sub));
        cx.notify();
    }

    /// Re-reads the preferences the UI uses (after the Preferences panel).
    fn reload_ui_prefs(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(client.get_preferences())
                .await;
            this.update(cx, |this, cx| {
                if this.generation == generation
                    && let Ok(p) = res
                {
                    this.ui_prefs = Some(p);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Refresh the torrent list right away (after adds / removes).
    fn refresh_now(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let list = cx.background_executor().spawn(client.list_torrents()).await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.apply_list(list, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_connection_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (dot, text): (gpui::Rgba, String) = match &self.conn {
            ConnState::Invalid(_) => (theme::error(), "Invalid URL".into()),
            ConnState::Connecting => (theme::warning(), "Connecting…".into()),
            ConnState::Connected => (
                theme::success(),
                format!(
                    "Connected · {} torrent{}",
                    self.torrents.len(),
                    if self.torrents.len() == 1 { "" } else { "s" }
                ),
            ),
            ConnState::Unreachable { .. } => (theme::error(), "Unreachable".into()),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                div()
                    .text_color(theme::text())
                    .font_weight(gpui::FontWeight::BOLD)
                    .pr_2()
                    .child("rqbit"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_muted())
                    .child("Server"),
            )
            .child(div().flex_1().max_w(px(520.)).child(self.url_input.clone()))
            .child(
                widgets::button("connect", "Connect", true).on_click(cx.listener(
                    |this, _, _, cx| {
                        let url = this.url_input.read(cx).text().to_owned();
                        this.connect(url, cx)
                    },
                )),
            )
            .child(div().flex_1())
            .child(
                widgets::primary_button("open-add", "Add", self.client.is_some()).when(
                    self.client.is_some(),
                    |b| {
                        b.on_click(cx.listener(|this, _, _, cx| {
                            this.open_add(cx);
                        }))
                    },
                ),
            )
            .child(self.render_events_button(cx))
            .child(
                widgets::button("open-prefs", "Preferences", self.client.is_some())
                    .when(self.client.is_some(), |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.open_prefs(cx)))
                    }),
            )
            .child(div().w(px(8.)))
            .child(div().size(px(8.)).rounded_full().bg(dot))
            .child(div().text_sm().text_color(theme::text_muted()).child(text))
    }

    /// "Events" button with a badge for unseen repairs + errors.
    fn render_events_button(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.client.is_some();
        let unseen = self.events_summary.as_ref().map(|s| s.unseen.clone());
        let count = unseen.as_ref().map_or(0, |u| u.repairs + u.errors);
        let tip: SharedString = match &unseen {
            Some(u) => format!(
                "Events: {} new repair(s), {} new error(s) since last viewed",
                u.repairs, u.errors
            )
            .into(),
            None => "Events: repairs & errors".into(),
        };
        let badge_bg = if unseen.as_ref().is_some_and(|u| u.errors > 0) {
            theme::error()
        } else {
            theme::warning()
        };
        div()
            .relative()
            .child(
                widgets::button("open-events", "Events", enabled)
                    .tooltip(widgets::text_tooltip(tip))
                    .when(enabled, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.open_events(None, cx)))
                    }),
            )
            .when(count > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(-6.))
                        .right(px(-8.))
                        .min_w(px(16.))
                        .h(px(16.))
                        .px_1()
                        .rounded_full()
                        .bg(badge_bg)
                        .text_color(gpui::white())
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if count > 99 {
                            "99+".to_owned()
                        } else {
                            count.to_string()
                        }),
                )
            })
    }

    fn render_banners(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let conn_error = match &self.conn {
            ConnState::Invalid(e) => Some(format!("Can't connect: {e}")),
            ConnState::Unreachable { error, since } => {
                let base = self
                    .client
                    .as_ref()
                    .map(|c| c.base_url().to_string())
                    .unwrap_or_default();
                let stale = if self.last_update.is_some() {
                    " Showing the last known list."
                } else {
                    ""
                };
                Some(format!(
                    "Cannot reach rqbit at {base}: {error}. Retrying every {}s (down for {}s).{stale}",
                    RETRY_INTERVAL.as_secs(),
                    since.elapsed().as_secs()
                ))
            }
            _ => None,
        };
        let banner = |id: &'static str, text: String| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .mx_3()
                .mt_2()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(theme::error())
                .bg(theme::error_bg())
                .text_sm()
                .text_color(theme::text())
                .child(div().flex_1().child(text))
        };
        div()
            .flex()
            .flex_col()
            .when_some(conn_error, |d, e| d.child(banner("conn-error", e)))
            .when_some(self.action_error.clone(), |d, e| {
                d.child(banner("action-error", e).child(
                    widgets::button("dismiss-error", "Dismiss", true).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.action_error = None;
                            cx.notify();
                        },
                    )),
                ))
            })
    }

    fn render_footer(&self) -> impl IntoElement {
        let (conn_text, conn_tip) = match &self.client {
            Some(c) => connection_label(c.base_url(), c.last_connection()),
            None => (String::new(), String::new()),
        };
        let (ip_text, ip_tip) = public_ip_label(self.public_ip.as_ref());
        let fmt = |mbps: f64| {
            let s = format_speed(mbps);
            if s.is_empty() {
                "0 Bytes/s".to_owned()
            } else {
                s
            }
        };
        let limit = |bps: Option<u64>| match bps {
            Some(b) if b > 0 => format!(" · limit {}/s", format_bytes(b)),
            _ => String::new(),
        };
        let (dl_limit, ul_limit) = self
            .limits
            .as_ref()
            .map_or((None, None), |l| (l.download_bps, l.upload_bps));
        let footer = div()
            .flex()
            .flex_row()
            .gap_4()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .text_xs()
            .text_color(theme::text_muted())
            .child(
                div()
                    .id("footer-conn")
                    .flex_1()
                    .truncate()
                    .tooltip(widgets::text_tooltip(conn_tip.into()))
                    .child(conn_text),
            )
            .when(!ip_text.is_empty(), |d| {
                d.child(
                    div()
                        .id("footer-public-ip")
                        .tooltip(widgets::text_tooltip(ip_tip.into()))
                        .child(ip_text),
                )
            });
        match &self.session_stats {
            // Web UI footer: speed (session total), uptime.
            Some(s) => footer
                .child(format!(
                    "↓ {} ({}){}",
                    fmt(s.download_speed.mbps),
                    format_bytes(s.counters.fetched_bytes),
                    limit(dl_limit)
                ))
                .child(format!(
                    "↑ {} ({}){}",
                    fmt(s.upload_speed.mbps),
                    format_bytes(s.counters.uploaded_bytes),
                    limit(ul_limit)
                ))
                .child(format!("{} peers", s.peers.live))
                .child(format!("up {}", format_uptime(s.uptime_seconds))),
            // Older server without /stats: sum the list.
            None => {
                let (down, up) = self
                    .torrents
                    .iter()
                    .filter_map(|t| t.stats.as_ref()?.live.as_ref())
                    .fold((0.0, 0.0), |(d, u), l| {
                        (d + l.download_speed.mbps, u + l.upload_speed.mbps)
                    });
                footer
                    .child(format!("↓ {}{}", fmt(down), limit(dl_limit)))
                    .child(format!("↑ {}{}", fmt(up), limit(ul_limit)))
            }
        }
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let has = !self.selection.is_empty() && !self.bulk_busy;
        let act = |id: &'static str, label: &'static str, action: TorrentAction| {
            widgets::button(id, label, has).when(has, |b| {
                b.on_click(cx.listener(move |this, _, _, cx| this.run_on_selection(action, cx)))
            })
        };
        let qbtn =
            |id: &'static str, label: &'static str, tip: &'static str, action: &'static str| {
                widgets::button(id, label, has)
                    .tooltip(widgets::text_tooltip(tip.into()))
                    .when(has, |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| this.queue_move(action, cx)))
                    })
            };
        let filter_label = format!("Status: {}", self.filter.label());
        let filter_menu = self.filter_menu_open.then(|| {
            deferred(
                div()
                    .id("filter-menu")
                    .absolute()
                    .top(px(28.))
                    .right_0()
                    .w(px(220.))
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.filter_menu_open = false;
                        cx.notify();
                    }))
                    .children(StatusFilter::ALL.iter().enumerate().map(|(i, f)| {
                        let f = *f;
                        let count = self.torrents.iter().filter(|t| f.matches(t)).count();
                        div()
                            .id(("filter-opt", i))
                            .flex()
                            .flex_row()
                            .px_3()
                            .py_1()
                            .text_sm()
                            .cursor_pointer()
                            .when(f == self.filter, |d| d.text_color(theme::primary()))
                            .hover(|s| s.bg(theme::surface_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.filter = f;
                                this.filter_menu_open = false;
                                cx.notify();
                            }))
                            .child(div().flex_1().child(f.label()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(count.to_string()),
                            )
                    })),
            )
            .with_priority(1)
        });
        let search_has_text = !self.search_input.read(cx).text().is_empty();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(theme::border())
            .child(act("bulk-start", "Resume", TorrentAction::Start))
            .child(act("bulk-pause", "Pause", TorrentAction::Pause))
            .child(act("bulk-restart", "Restart", TorrentAction::Restart))
            .child(act("bulk-fix", "Fix errors", TorrentAction::FixErrors))
            .child(div().w(px(6.)))
            .child(qbtn("q-top", "Top", "Move to top of queue", "top"))
            .child(qbtn("q-up", "↑", "Move up in queue", "up"))
            .child(qbtn("q-down", "↓", "Move down in queue", "down"))
            .child(qbtn(
                "q-bottom",
                "Bottom",
                "Move to bottom of queue",
                "bottom",
            ))
            .child(div().w(px(6.)))
            .child(
                widgets::button("bulk-delete", "Delete", has).when(has, |b| {
                    b.text_color(theme::error())
                        .on_click(cx.listener(|this, _, _, cx| this.ask_delete(cx)))
                }),
            )
            .when(!self.selection.is_empty(), |d| {
                d.child(
                    div()
                        .pl_2()
                        .text_sm()
                        .text_color(theme::text_muted())
                        .child(format!("{} selected", self.selection.len())),
                )
                .child(widgets::link("clear-sel", "clear").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.selection.clear();
                        cx.notify();
                    },
                )))
            })
            .child(div().flex_1())
            .child(
                div()
                    .relative()
                    .child(
                        widgets::button("filter-btn", filter_label, true)
                            .when(self.filter != StatusFilter::All, |b| {
                                b.border_color(theme::primary())
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.filter_menu_open = !this.filter_menu_open;
                                cx.notify();
                            })),
                    )
                    .children(filter_menu),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .w(px(220.))
                    .child(div().flex_1().child(self.search_input.clone()))
                    .when(search_has_text, |d| {
                        d.child(
                            widgets::button("search-clear", "×", true).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.search_input.update(cx, |i, cx| i.set_text("", cx));
                                },
                            )),
                        )
                    }),
            )
    }

    fn render_confirm(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.confirm_delete.as_ref()?;
        let n = dialog.items.len();
        let delete_files = dialog.delete_files;
        let title = if n == 1 {
            "Delete torrent?".to_owned()
        } else {
            format!("Delete {n} torrents?")
        };
        let shown: Vec<SharedString> = dialog
            .items
            .iter()
            .take(12)
            .map(|(_, n)| n.clone())
            .collect();
        let more = n.saturating_sub(shown.len());
        Some(
            div()
                .id("confirm-overlay")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::overlay())
                .occlude()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .w(px(520.))
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .text_color(theme::text())
                        .child(div().font_weight(gpui::FontWeight::BOLD).child(title))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .text_sm()
                                .children(shown.into_iter().map(|n| div().truncate().child(n)))
                                .when(more > 0, |d| {
                                    d.child(
                                        div()
                                            .text_color(theme::text_muted())
                                            .child(format!("…and {more} more")),
                                    )
                                }),
                        )
                        .child(
                            widgets::checkbox(
                                "delete-files",
                                "Also delete downloaded files",
                                delete_files,
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(d) = &mut this.confirm_delete {
                                    d.delete_files = !d.delete_files;
                                }
                                cx.notify();
                            })),
                        )
                        .child(div().text_xs().text_color(theme::text_muted()).child(
                            if delete_files {
                                "The torrents are removed from rqbit and their files are deleted from disk."
                            } else {
                                "The torrents are removed from rqbit. Downloaded files are kept on disk."
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap_2()
                                .child(widgets::button("confirm-cancel", "Cancel", true).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.confirm_delete = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    widgets::danger_button(
                                        "confirm-remove",
                                        if delete_files { "Delete with files" } else { "Delete" },
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(d) = this.confirm_delete.take() {
                                            let ids = d.items.iter().map(|(id, _)| *id).collect();
                                            let action = if d.delete_files {
                                                TorrentAction::Delete
                                            } else {
                                                TorrentAction::Forget
                                            };
                                            this.run_action(ids, action, true, cx);
                                        }
                                    })),
                                ),
                        ),
                ),
        )
    }
}

/// `host:port` with the scheme's default port made explicit; IPv6 literals
/// in brackets.
fn host_port(url: &url::Url) -> String {
    let host = match url.host() {
        Some(url::Host::Ipv6(a)) => format!("[{a}]"),
        Some(h) => h.to_string(),
        None => String::new(),
    };
    match url.port_or_known_default() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    }
}

/// Footer connection text + tooltip, from the client's own view of its
/// connection: native shows `local <--> remote` of the socket that carried
/// the last response (the resolved address, not the hostname); the browser
/// can't see either socket address, so only the origin's host:port.
fn connection_label(base: &url::Url, conn: Option<api::ConnEndpoints>) -> (String, String) {
    let target = host_port(base);
    match conn {
        Some(c) => {
            let remote = c.remote.to_string(); // [v6]:port for IPv6
            let text = match c.local {
                Some(l) => format!("{l} <--> {remote}"),
                None => remote.clone(),
            };
            (
                text,
                format!(
                    "{base} → {target} resolved to {remote}{}",
                    c.local
                        .map(|l| format!(", from local {l}"))
                        .unwrap_or_default()
                ),
            )
        }
        None if cfg!(target_family = "wasm") => (
            target,
            format!(
                "{base}\nBrowsers don't reveal the resolved server IP or the local \
                 port of a connection to web pages, so only the host is shown."
            ),
        ),
        None => (target, format!("{base} (not connected yet)")),
    }
}

/// Footer public IP text + tooltip (`GET /public_ip`).
fn public_ip_label(p: Option<&api::PublicIp>) -> (String, String) {
    let Some(p) = p else {
        return (String::new(), String::new());
    };
    if !p.enabled {
        return (String::new(), String::new());
    }
    let mut parts = Vec::new();
    if let Some(ip) = &p.ipv4.ip {
        parts.push(ip.clone());
    }
    if let Some(ip) = &p.ipv6.ip {
        parts.push(ip.clone());
    }
    let text = if parts.is_empty() {
        if p.checked_at.is_none() {
            "public IP: checking…".to_owned()
        } else {
            "public IP: unknown".to_owned()
        }
    } else {
        format!("public {}", parts.join(" · "))
    };
    let fam = |name: &str, f: &api::PublicIpFamily| match (&f.ip, &f.error) {
        (Some(ip), _) => format!(
            "{name}: {ip}{}",
            f.source
                .as_ref()
                .map(|s| format!(" (via {s})"))
                .unwrap_or_default()
        ),
        (None, Some(e)) => format!("{name}: none ({e})"),
        (None, None) => format!("{name}: not checked"),
    };
    let checked = match (&p.checked_at, p.age_secs) {
        (Some(t), Some(a)) => format!("checked {} ({a}s ago)", details::format_event_time(t)),
        (Some(t), None) => format!("checked {}", details::format_event_time(t)),
        _ => "not checked yet".to_owned(),
    };
    (
        text,
        format!(
            "Server's public address (its own egress, e.g. the VPN exit)\n{}\n{}\n{checked}",
            fam("IPv4", &p.ipv4),
            fam("IPv6", &p.ipv6)
        ),
    )
}

impl Render for RqbitWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_input.read(cx).text().to_owned();
        self.visible = list_state::visible_rows(
            &self.torrents,
            &query,
            self.filter,
            self.sort,
            self.sort_dir,
        );
        let count = self.visible.len();
        let total = self.torrents.len();
        let connected = matches!(self.conn, ConnState::Connected);
        let visible_ids = self.visible_ids();
        let all_selected =
            !visible_ids.is_empty() && visible_ids.iter().all(|id| self.selection.contains(*id));
        let some_selected = visible_ids.iter().any(|id| self.selection.contains(*id));
        let header_state = torrent_table::HeaderState {
            sort: self.sort,
            dir: self.sort_dir,
            all_selected,
            some_selected,
        };
        let empty_text = if !connected {
            None
        } else if total == 0 {
            Some("No torrents. Use Add to get started.".to_owned())
        } else if count == 0 {
            Some(format!(
                "No torrents match the current filter ({total} hidden)."
            ))
        } else {
            None
        };
        let details = self.sync_details(cx);
        let root = div()
            .id("root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::bg())
            .text_color(theme::text())
            .child(self.render_connection_bar(cx))
            .child(self.render_banners(cx))
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .px_3()
                    .child(torrent_table::header(header_state, cx))
                    .when_some(empty_text, |d, t| {
                        d.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(theme::text_muted())
                                .child(t),
                        )
                    })
                    .child(
                        uniform_list(
                            "torrents",
                            count,
                            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                                // Focus ring only while the list has keyboard focus.
                                let cursor = this
                                    .focus_handle
                                    .is_focused(window)
                                    .then(|| this.selection.focus())
                                    .flatten();
                                range
                                    .filter_map(|ix| {
                                        let t = this.torrents.get(*this.visible.get(ix)?)?;
                                        let state = torrent_table::RowState {
                                            selected: this.selection.contains(t.id),
                                            focused: cursor == Some(t.id),
                                            pending: this.pending.contains(&t.id),
                                        };
                                        Some(torrent_table::row(t, ix, state, cx))
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .with_scroll(&self.list_scroll)
                        .flex_1(),
                    ),
            )
            .when_some(details, |d, p| {
                d.child(div().h(px(330.)).flex_shrink_0().child(p))
            })
            .child(self.render_footer())
            .when_some(self.add_panel.as_ref().map(|(p, _)| p.clone()), |d, p| {
                d.child(widgets::modal("add-overlay", 820., p))
            })
            .when_some(self.prefs_panel.as_ref().map(|(p, _)| p.clone()), |d, p| {
                d.child(widgets::modal("prefs-overlay", 760., p))
            })
            .when_some(
                self.events_panel.as_ref().map(|(p, _)| p.clone()),
                |d, p| d.child(widgets::modal("events-overlay", 1000., p)),
            )
            .children(self.render_confirm(cx));
        #[cfg(not(target_family = "wasm"))]
        let root = root.on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
            let paths = paths.paths().to_vec();
            let Some(panel) = this.open_add(cx) else {
                return;
            };
            cx.spawn(async move |_, cx| {
                let read = cx
                    .background_executor()
                    .spawn(async move { files::read_paths(&paths) })
                    .await;
                panel
                    .update(cx, |p, cx| p.add_read_results(read, "drop", cx))
                    .ok();
            })
            .detach();
        }));
        root
    }
}

/// `UniformList::track_scroll` takes the handle by reference after gpui 0.2.2.
trait TrackScrollCompat {
    fn with_scroll(self, handle: &UniformListScrollHandle) -> Self;
}

impl TrackScrollCompat for gpui::UniformList {
    #[cfg(not(feature = "gpui-main"))]
    fn with_scroll(self, handle: &UniformListScrollHandle) -> Self {
        self.track_scroll(handle.clone())
    }
    #[cfg(feature = "gpui-main")]
    fn with_scroll(self, handle: &UniformListScrollHandle) -> Self {
        self.track_scroll(handle)
    }
}

#[cfg(test)]
mod footer_tests {
    use super::*;

    #[test]
    fn host_ports() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert_eq!(host_port(&u("https://r.witherow.ca/")), "r.witherow.ca:443");
        assert_eq!(host_port(&u("http://10.0.0.2:9030/")), "10.0.0.2:9030");
        assert_eq!(host_port(&u("http://[::1]:3030/")), "[::1]:3030");
        assert_eq!(host_port(&u("http://example.com/")), "example.com:80");
    }

    #[test]
    fn native_connection_uses_socket_addrs() {
        let base = url::Url::parse("https://r.example/").unwrap();
        let c = api::ConnEndpoints {
            local: Some("192.168.0.30:54321".parse().unwrap()),
            remote: "[2001:db8::5]:443".parse().unwrap(),
        };
        let (text, tip) = connection_label(&base, Some(c));
        assert_eq!(text, "192.168.0.30:54321 <--> [2001:db8::5]:443");
        assert!(tip.contains("r.example:443"), "{tip}");
        let c = api::ConnEndpoints {
            local: None,
            remote: "192.168.0.101:9030".parse().unwrap(),
        };
        assert_eq!(connection_label(&base, Some(c)).0, "192.168.0.101:9030");
    }

    #[test]
    fn public_ip_text() {
        assert_eq!(public_ip_label(None).0, "");
        let p = api::PublicIp {
            enabled: true,
            ipv4: api::PublicIpFamily {
                ip: Some("203.0.113.7".into()),
                ..Default::default()
            },
            ipv6: api::PublicIpFamily {
                error: Some("can't connect".into()),
                ..Default::default()
            },
            checked_at: Some("2026-01-01T00:00:00Z".into()),
            age_secs: Some(3),
            checking: false,
        };
        let (t, tip) = public_ip_label(Some(&p));
        assert_eq!(t, "public 203.0.113.7");
        assert!(tip.contains("IPv6: none (can't connect)"), "{tip}");
    }
}
