//! GPUI views.
//!
//! `RqbitWindow` is the root view: connection bar, status/error banners, the
//! torrent table and a confirm dialog. New web-UI features should be added as
//! their own modules/views here (e.g. `details.rs`, `add_torrent.rs`,
//! `events.rs`) with any new endpoints going into `crate::api`.

mod add_panel;
mod category;
mod context_menu;
mod details;
mod cleanup_panel;
mod events_panel;
mod files;
mod list_state;
mod menus;
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
use cleanup_panel::{CleanupPanel, CleanupPanelEvent};
use events_panel::{EventsPanel, EventsPanelEvent};
use list_state::{FixAction, Nav, Selection, SortColumn, SortDir, StatusFilter};
use prefs_panel::{PrefsPanel, PrefsPanelEvent};
use text_input::{TextInput, TextInputEvent};

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
    /// `POST /remove` with an explicit policy.
    Remove(api::RemovePolicy),
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
            TorrentAction::Remove(_) => "Remove",
        }
    }
}

/// Open right-click menu on torrent rows.
struct RowMenu {
    pos: gpui::Point<gpui::Pixels>,
    ids: Vec<usize>,
    open_sub: Option<usize>,
    /// Download order of a single right-clicked torrent (marks the menu).
    order: Option<api::DownloadOrderView>,
}

/// "Set category…" for the right-clicked torrents.
struct CategoryDialog {
    ids: Vec<usize>,
    init: [category::FieldInit; 4],
    inputs: Vec<Entity<TextInput>>,
    error: Option<String>,
    busy: bool,
    _subs: Vec<Subscription>,
}

struct DeleteDialog {
    items: Vec<(usize, SharedString)>,
    policy: api::RemovePolicy,
    /// Complete / incomplete counts (None while loading or on old servers).
    preview: Option<api::RemovePreview>,
    loading: bool,
}

/// `store` key: "never" = don't offer to become the default handler.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
const HANDLER_PROMPT_KEY: &str = "handler_prompt";

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", allow(dead_code))]
enum HandlerPrompt {
    Ask,
    Working,
    Done(String),
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
    row_menu: Option<RowMenu>,
    /// "Move files…" from the menu: open the details' move form once shown.
    pending_move: Option<usize>,
    selection: Selection,
    /// Scroll position of the torrent list (keeps the keyboard cursor in view).
    list_scroll: UniformListScrollHandle,
    filter: StatusFilter,
    filter_menu_open: bool,
    category_filter: category::CategoryFilter,
    category_menu_open: bool,
    category_dialog: Option<CategoryDialog>,
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
    cleanup_panel: Option<(Entity<CleanupPanel>, Subscription)>,
    /// First-run "make rqbit the default" bar (native).
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    handler_prompt: Option<HandlerPrompt>,
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
            row_menu: None,
            pending_move: None,
            selection: Selection::default(),
            list_scroll: UniformListScrollHandle::new(),
            filter: StatusFilter::All,
            filter_menu_open: false,
            category_filter: Default::default(),
            category_menu_open: false,
            category_dialog: None,
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
            cleanup_panel: None,
            handler_prompt: None,
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
        // Magnets / .torrent files the app was opened with (or forwarded by a
        // second launch, or the browser's #add= fragment).
        #[cfg(target_family = "wasm")]
        crate::launch::queue_page_fragment();
        if let Some(mut rx) = crate::launch::take_receiver() {
            cx.spawn_in(window, async move |this, cx| {
                use futures::StreamExt;
                while let Some(items) = rx.next().await {
                    if this
                        .update_in(cx, |this, window, cx| {
                            window.activate_window();
                            cx.activate(true);
                            this.open_launch_items(items, cx);
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            })
            .detach();
        }
        #[cfg(not(target_family = "wasm"))]
        this.check_default_handler(cx);
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
        self.category_dialog = None;
        self.selection.clear();
        self.details = None;
        self.poll_task = None;
        self.client = None;
        self.events_panel = None;
        self.cleanup_panel = None;
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
                #[cfg(not(target_family = "wasm"))]
                crate::store::set("last_url", raw.trim());
                let client = ApiClient::new(self.transport.clone(), url);
                self.client = Some(client.clone());
                self.conn = ConnState::Connecting;
                let generation = self.generation;
                let client_for_stats = client.clone();
                // Torrent list: WebSocket snapshot + deltas, else delta polling
                // (crate::feed); one full list per update, as before.
                let mut feed = crate::feed::transport::TorrentFeed::new(client.clone());
                self.poll_task = Some(cx.spawn(async move |this, cx| {
                    let bg = cx.background_executor().clone();
                    loop {
                        let res = feed
                            .next(&bg)
                            .await
                            .map(|torrents| ListTorrentsResponse { torrents });
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
                        if !ok {
                            cx.background_executor().timer(RETRY_INTERVAL).await;
                        }
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
                    TorrentAction::Remove(p) => {
                        let f = client.remove(id, Some(p));
                        futures::FutureExt::boxed(async move { f.await.map(|_| ()) })
                    }
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
        let plan = list_state::plan_remove(self.ui_prefs.as_ref(), None);
        let ids: Vec<usize> = items.iter().map(|(id, _)| *id).collect();
        self.confirm_delete = Some(DeleteDialog {
            items,
            policy: plan.policy,
            preview: None,
            loading: true,
        });
        cx.notify();
        let Some(client) = self.client.clone() else {
            return;
        };
        // Which torrents are complete decides what the policy does (and
        // whether the dialog can be skipped).
        cx.spawn(async move |this, cx| {
            let pv = cx
                .background_executor()
                .spawn(client.remove_preview(&ids))
                .await;
            this.update(cx, |this, cx| {
                let Some(d) = &mut this.confirm_delete else { return };
                d.loading = false;
                match pv {
                    Ok(pv) => {
                        let mut prefs = this.ui_prefs.clone().unwrap_or_default();
                        prefs.insert("confirm_remove".into(), pv.confirm_remove.into());
                        if let Some(p) = &pv.policy {
                            prefs.insert("remove_policy".into(), p.clone());
                        }
                        let plan =
                            list_state::plan_remove(Some(&prefs), Some((pv.complete, pv.incomplete)));
                        d.policy = plan.policy;
                        d.preview = Some(pv);
                        if !plan.confirm {
                            let ids = d.items.iter().map(|(id, _)| *id).collect();
                            this.confirm_delete = None;
                            this.run_action(ids, TorrentAction::Remove(plan.policy), true, cx);
                            return;
                        }
                    }
                    Err(e) => {
                        // Old server without the preview: keep the dialog.
                        log::debug!("remove preview failed: {e:#}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    /// Right-click on a row: acts on the selection if the row is part of it,
    /// otherwise selects just that row.
    pub(crate) fn row_context_menu(
        &mut self,
        id: usize,
        pos: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        text_input::focus(window, &self.focus_handle, cx);
        if !self.selection.contains(id) {
            self.selection.select_one(id);
        }
        let ids = self.selected_in_order();
        let single = (ids.len() == 1).then(|| ids[0]);
        self.row_menu = Some(RowMenu {
            pos,
            ids,
            open_sub: None,
            order: None,
        });
        cx.notify();
        if let (Some(id), Some(client)) = (single, self.client.clone()) {
            cx.spawn(async move |this, cx| {
                let r = cx
                    .background_executor()
                    .spawn(client.get_download_order(id))
                    .await;
                this.update(cx, |this, cx| {
                    if let (Ok(v), Some(m)) = (r, &mut this.row_menu)
                        && m.ids == [id]
                    {
                        m.order = Some(v);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn row_menu_action(&mut self, action: menus::RowMenuAction, cx: &mut Context<Self>) {
        use menus::RowMenuAction as R;
        let Some(ids) = self.row_menu.take().map(|m| m.ids) else {
            return;
        };
        match action {
            R::Details => {
                if let Some(id) = ids.first() {
                    self.open_details(*id, cx);
                }
            }
            R::Act(a) => self.run_action(ids, a, false, cx),
            R::Queue(a) => self.queue_move(a, cx),
            R::Move => {
                if let Some(id) = ids.first().copied() {
                    self.open_details(id, cx);
                    self.pending_move = Some(id);
                }
            }
            R::Remove => self.ask_delete(cx),
            R::SetCategory => self.open_category_dialog(ids, cx),
            R::Order(patch) => {
                let Some(client) = self.client.clone() else {
                    return;
                };
                cx.spawn(async move |this, cx| {
                    let mut errors = Vec::new();
                    for id in ids {
                        let r = cx
                            .background_executor()
                            .spawn(client.set_download_order(id, &patch))
                            .await;
                        if let Err(e) = r {
                            errors.push(format!("#{id}: {e:#}"));
                        }
                    }
                    this.update(cx, |this, cx| {
                        if !errors.is_empty() {
                            this.action_error =
                                Some(format!("Download order failed: {}", errors.join("; ")));
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn open_category_dialog(&mut self, ids: Vec<usize>, cx: &mut Context<Self>) {
        let items: Vec<&TorrentListItem> = ids
            .iter()
            .filter_map(|id| self.torrents.iter().find(|t| t.id == *id))
            .collect();
        if items.is_empty() {
            return;
        }
        let ids: Vec<usize> = items.iter().map(|t| t.id).collect();
        let init = category::initial_form(&items);
        let mut subs = Vec::new();
        let inputs = category::CategoryField::ALL
            .iter()
            .zip(init.iter())
            .map(|(f, i)| {
                let placeholder = if i.mixed { "(mixed, unchanged)" } else { f.placeholder() };
                let input = cx.new(|cx| TextInput::new(i.value.clone(), placeholder, cx));
                subs.push(cx.subscribe(&input, |this, _, ev: &TextInputEvent, cx| match ev {
                    TextInputEvent::Submit(_) => this.apply_category(false, cx),
                    TextInputEvent::Changed => {}
                }));
                input
            })
            .collect();
        self.category_dialog = Some(CategoryDialog {
            ids,
            init,
            inputs,
            error: None,
            busy: false,
            _subs: subs,
        });
        cx.notify();
    }

    /// Save the dialog (`clear`: clear every part) for each torrent.
    fn apply_category(&mut self, clear: bool, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(d) = self.category_dialog.as_mut() else {
            return;
        };
        if d.busy {
            return;
        }
        let update = if clear {
            category::clear_update()
        } else {
            let current: [String; 4] =
                std::array::from_fn(|i| d.inputs[i].read(cx).text().to_owned());
            match category::build_update(&d.init, &current) {
                Ok(u) => u,
                Err(e) => {
                    d.error = Some(e);
                    cx.notify();
                    return;
                }
            }
        };
        if update.as_object().is_some_and(|m| m.is_empty()) {
            self.category_dialog = None;
            cx.notify();
            return;
        }
        d.busy = true;
        d.error = None;
        let ids = d.ids.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut errors = Vec::new();
            for id in ids {
                let r = cx
                    .background_executor()
                    .spawn(client.set_category(id, &update))
                    .await;
                if let Err(e) = r {
                    errors.push(format!("#{id}: {e:#}"));
                }
            }
            this.update(cx, |this, cx| {
                if errors.is_empty() {
                    this.category_dialog = None;
                } else if let Some(d) = this.category_dialog.as_mut() {
                    d.busy = false;
                    d.error = Some(errors.join("\n"));
                }
                this.refresh_now(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_category_dialog(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let d = self.category_dialog.as_ref()?;
        let n = d.ids.len();
        let title = if n > 1 {
            format!("Set category ({n} torrents)")
        } else {
            "Set category".to_owned()
        };
        let busy = d.busy;
        let rows: Vec<_> = category::CategoryField::ALL
            .iter()
            .zip(d.inputs.iter())
            .map(|(f, input)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(120.)).text_sm().child(f.label()))
                    .child(div().flex_1().child(input.clone()))
            })
            .collect();
        Some(
            div()
                .id("category-overlay")
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
                        .children(rows)
                        .child(div().text_xs().text_color(theme::text_muted()).child(
                            "Only changed fields are saved; an emptied field is cleared. Without a Torznab number, auto-organize looks the source and id up. Affects future organizing only; nothing is moved now.",
                        ))
                        .when_some(d.error.clone(), |el, e| {
                            el.child(div().text_xs().text_color(theme::error()).child(e))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(
                                    widgets::button("category-clear", "Clear category", !busy).when(!busy, |b| {
                                        b.on_click(cx.listener(|this, _, _, cx| this.apply_category(true, cx)))
                                    }),
                                )
                                .child(div().flex_1())
                                .child(widgets::button("category-cancel", "Cancel", true).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.category_dialog = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    widgets::button("category-save", "Save", !busy)
                                        .border_color(theme::primary())
                                        .when(!busy, |b| {
                                            b.on_click(cx.listener(|this, _, _, cx| this.apply_category(false, cx)))
                                        }),
                                ),
                        ),
                ),
        )
    }

    fn render_row_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let m = self.row_menu.as_ref()?;
        let rows: Vec<menus::RowInfo> = m
            .ids
            .iter()
            .map(|id| {
                let stats = self.torrents.iter().find(|t| t.id == *id).and_then(|t| t.stats.as_ref());
                menus::RowInfo {
                    state: stats.map(|s| s.state),
                    fixable: list_state::fix_action(stats) != FixAction::Skip,
                }
            })
            .collect();
        let entries = menus::torrent_menu(&rows, m.order.as_ref(), true);
        Some(context_menu::render_menu(
            m.pos,
            window.viewport_size(),
            &entries,
            m.open_sub,
            context_menu::MenuHost {
                on_action: |this: &mut Self, a, cx| this.row_menu_action(a, cx),
                on_sub: |this: &mut Self, sub, cx| {
                    if let Some(m) = &mut this.row_menu
                        && m.open_sub != sub
                    {
                        m.open_sub = sub;
                        cx.notify();
                    }
                },
                on_close: |this: &mut Self, cx| {
                    if this.row_menu.take().is_some() {
                        cx.notify();
                    }
                },
            },
            cx,
        ))
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
        let open_move = self.pending_move.take_if(|m| *m == id).is_some();
        panel.update(cx, |p, cx| {
            p.set_torrent(t);
            if open_move {
                p.open_move(cx);
            }
        });
        Some(panel)
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Escape closes an open right-click menu or the remove dialog first
        // (and only that), wherever focus is.
        if ev.keystroke.key == "escape" {
            // The remove dialog is also cancelled by Escape (never confirmed).
            let mut closed = self.row_menu.take().is_some()
                || self.confirm_delete.take().is_some()
                || self.category_dialog.take().is_some();
            if let Some((panel, _)) = &self.details {
                closed |= panel.update(cx, |p, cx| p.close_menu(cx));
            }
            if closed {
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
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
        self.cleanup_panel = None;
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
        self.cleanup_panel = None;
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

    /// Shows launch items (magnets, .torrent files) in the Add panel.
    fn open_launch_items(&mut self, items: Vec<crate::launch::LaunchItem>, cx: &mut Context<Self>) {
        use crate::launch::LaunchItem;
        if items.is_empty() {
            return;
        }
        let Some(panel) = self.open_add(cx) else {
            return;
        };
        let mut links = Vec::new();
        #[allow(unused_mut)]
        let mut paths = Vec::new();
        for i in items {
            match i {
                LaunchItem::Link(l) => links.push(l),
                LaunchItem::File(p) => paths.push(p),
            }
        }
        if !links.is_empty() {
            panel.update(cx, |p, cx| p.stage_links(links, "open", cx));
        }
        #[cfg(not(target_family = "wasm"))]
        if !paths.is_empty() {
            cx.spawn(async move |_, cx| {
                let read = cx
                    .background_executor()
                    .spawn(async move { files::read_paths(&paths) })
                    .await;
                panel
                    .update(cx, |p, cx| p.add_read_results(read, "open", cx))
                    .ok();
            })
            .detach();
        }
        #[cfg(target_family = "wasm")]
        let _ = paths;
    }

    /// First run: offer to become the default for magnet links / .torrent
    /// files unless it already is or the user said "don't ask again".
    #[cfg(not(target_family = "wasm"))]
    fn check_default_handler(&mut self, cx: &mut Context<Self>) {
        if crate::store::get(HANDLER_PROMPT_KEY).as_deref() == Some("never") || cfg!(target_os = "macos") {
            return;
        }
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async move {
                    crate::handlers::register::Launcher::current()
                        .ok()
                        .map(|l| crate::handlers::register::status(&l))
                })
                .await;
            this.update(cx, |this, cx| {
                if status.is_some_and(|s| !s.is_default()) {
                    this.handler_prompt = Some(HandlerPrompt::Ask);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    #[cfg(not(target_family = "wasm"))]
    fn make_default_handler(&mut self, cx: &mut Context<Self>) {
        self.handler_prompt = Some(HandlerPrompt::Working);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    use crate::handlers::register as reg;
                    let l = reg::Launcher::current().map_err(|e| e.to_string())?;
                    let notes = reg::register(&l)?;
                    if cfg!(windows) {
                        reg::open_default_apps_settings();
                    }
                    Ok::<_, String>((notes, reg::status(&l)))
                })
                .await;
            this.update(cx, |this, cx| {
                this.handler_prompt = Some(HandlerPrompt::Done(match r {
                    Ok((_, s)) if s.is_default() => s.describe(),
                    Ok(_) if cfg!(windows) => "rqbit is registered. Windows doesn't let apps make themselves the default: in the Settings page that opened, choose rqbit for MAGNET and .torrent.".into(),
                    Ok((_, s)) => s.describe(),
                    Err(e) => format!("Couldn't register: {e}"),
                }));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    #[cfg(not(target_family = "wasm"))]
    fn render_handler_prompt(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let p = self.handler_prompt.clone()?;
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_sm()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface());
        let row = match p {
            HandlerPrompt::Ask => row
                .child(div().flex_1().child(
                    "rqbit isn't your default app for magnet links and .torrent files.",
                ))
                .child(
                    widgets::primary_button("hp-yes", "Make rqbit the default", true)
                        .on_click(cx.listener(|this, _, _, cx| this.make_default_handler(cx))),
                )
                .child(widgets::button("hp-later", "Not now", true).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.handler_prompt = None;
                        cx.notify();
                    },
                )))
                .child(widgets::button("hp-never", "Don't ask again", true).on_click(
                    cx.listener(|this, _, _, cx| {
                        crate::store::set(HANDLER_PROMPT_KEY, "never");
                        this.handler_prompt = None;
                        cx.notify();
                    }),
                )),
            HandlerPrompt::Working => row.child("Registering…"),
            HandlerPrompt::Done(msg) => row.child(div().flex_1().child(msg)).child(
                widgets::button("hp-close", "×", true).on_click(cx.listener(|this, _, _, cx| {
                    this.handler_prompt = None;
                    cx.notify();
                })),
            ),
        };
        Some(row.into_any_element())
    }

    fn open_cleanup(&mut self, cx: &mut Context<Self>) {
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
        self.events_panel = None;
        let panel = cx.new(|cx| CleanupPanel::new(client, cx));
        let sub = cx.subscribe(&panel, |this, _, ev: &CleanupPanelEvent, cx| match ev {
            CleanupPanelEvent::Close => {
                this.cleanup_panel = None;
                cx.notify();
            }
        });
        self.cleanup_panel = Some((panel, sub));
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
        self.cleanup_panel = None;
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
                widgets::button("open-cleanup", "Cleanup", self.client.is_some())
                    .tooltip(widgets::text_tooltip("Clean up orphaned downloads".into()))
                    .when(self.client.is_some(), |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.open_cleanup(cx)))
                    }),
            )
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
        let category_label = format!("Category: {}", self.category_filter.label());
        let category_menu = self.category_menu_open.then(|| {
            let mut options = vec![category::CategoryFilter::All, category::CategoryFilter::None];
            options.extend(
                category::category_options(&self.torrents)
                    .into_iter()
                    .map(category::CategoryFilter::Label),
            );
            if !options.contains(&self.category_filter) {
                options.push(self.category_filter.clone());
            }
            deferred(
                div()
                    .id("category-menu")
                    .absolute()
                    .top(px(28.))
                    .right_0()
                    .w(px(260.))
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.category_menu_open = false;
                        cx.notify();
                    }))
                    .children(options.into_iter().enumerate().map(|(i, f)| {
                        let count = self.torrents.iter().filter(|t| f.matches(t)).count();
                        let label = f.label();
                        let current = f == self.category_filter;
                        div()
                            .id(("category-opt", i))
                            .flex()
                            .flex_row()
                            .px_3()
                            .py_1()
                            .text_sm()
                            .cursor_pointer()
                            .when(current, |d| d.text_color(theme::primary()))
                            .hover(|s| s.bg(theme::surface_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.category_filter = f.clone();
                                this.category_menu_open = false;
                                cx.notify();
                            }))
                            .child(div().flex_1().truncate().child(label))
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
                        widgets::button("category-filter-btn", category_label, true)
                            .when(self.category_filter != category::CategoryFilter::All, |b| {
                                b.border_color(theme::primary())
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.category_menu_open = !this.category_menu_open;
                                this.filter_menu_open = false;
                                cx.notify();
                            })),
                    )
                    .children(category_menu),
            )
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
        let policy = dialog.policy;
        let pv = dialog.preview.clone();
        let loading = dialog.loading;
        let known = pv.is_some();
        let (n_c, n_i) = pv.as_ref().map(|p| (p.complete, p.incomplete)).unwrap_or((0, 0));
        let deletes = if known {
            policy.deletes_files(n_c > 0, n_i > 0)
        } else {
            policy.deletes_files(true, true)
        };
        let title = if n == 1 {
            "Remove torrent".to_owned()
        } else {
            format!("Remove {n} torrents")
        };
        let shown: Vec<(SharedString, Option<String>)> = dialog
            .items
            .iter()
            .take(12)
            .map(|(id, name)| {
                let tag = pv.as_ref().and_then(|p| p.items.iter().find(|i| i.id == *id)).map(|i| {
                    if i.complete {
                        "complete".to_owned()
                    } else {
                        format!(
                            "incomplete: {} done ({}), {} unfinished",
                            i.files_complete,
                            format_bytes(i.bytes_complete),
                            i.files_partial
                        )
                    }
                });
                (name.clone(), tag)
            })
            .collect();
        let more = n.saturating_sub(shown.len());
        let complete_text = match policy.complete {
            "delete" => "remove and delete their files",
            _ => "remove, keep files on disk",
        };
        let nothing_done = pv.as_ref().map(|p| p.incomplete_nothing_done).unwrap_or(0);
        // "Move files individually as they complete": the finished files have already
        // moved; nothing to ask about moving them.
        let individual = pv.as_ref().is_some_and(|p| p.files_move_individually);
        let incomplete_text = match policy.incomplete {
            "delete" => "remove and delete all their files (including finished ones)".to_owned(),
            "finish" if individual => "remove and delete the unfinished files; finished files are kept (they have already moved)".to_owned(),
            "finish" => {
                let mut t = "finish what's done: delete unfinished files, run completion actions on the finished ones, then remove".to_owned();
                if nothing_done > 0 {
                    if nothing_done == n_i {
                        t.push_str(". Nothing is complete yet, so all partial data is deleted and the torrent removed");
                    } else {
                        t.push_str(&format!(". {nothing_done} of them have no complete file yet: all their partial data is deleted"));
                    }
                }
                t
            }
            _ => "remove, keep partial files on disk".to_owned(),
        };
        let fwd = !individual && policy.incomplete == "finish" && (!known || n_i > 0);
        let actions = pv
            .as_ref()
            .map(|p| p.completion_actions.join(" → "))
            .unwrap_or_default();
        let legacy = !known && !loading;

        let seg_row = |label: String,
                       options: &'static [(&'static str, &'static str)],
                       current: &'static str,
                       enabled: bool,
                       base: usize,
                       complete: bool,
                       desc: String,
                       cx: &mut Context<Self>| {
            let last = options.len() - 1;
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(120.)).text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child(label))
                        .child(div().flex().flex_row().children(options.iter().enumerate().map(
                            |(k, (value, text))| {
                                let value: &'static str = value;
                                widgets::segment(("rm-policy", base + k), *text, current == value)
                                    .when(k == 0, |d| d.rounded_l_md())
                                    .when(k == last, |d| d.rounded_r_md())
                                    .when(!enabled, |d| d.opacity(0.4))
                                    .when(enabled, |d| {
                                        d.on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(d) = &mut this.confirm_delete {
                                                if complete {
                                                    d.policy.complete = value;
                                                } else {
                                                    d.policy.incomplete = value;
                                                }
                                            }
                                            cx.notify();
                                        }))
                                    })
                            },
                        ))),
                )
                .when(enabled, |d| {
                    d.child(
                        div()
                            .pl(px(128.))
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(format!("→ {desc}")),
                    )
                })
        };
        let complete_row = seg_row(
            if known { format!("{n_c} complete") } else { "Complete".into() },
            &[("keep", "Keep files"), ("delete", "Delete files")],
            policy.complete,
            !known || n_c > 0,
            0,
            true,
            complete_text.to_owned(),
            cx,
        );
        let incomplete_row = seg_row(
            if known { format!("{n_i} incomplete") } else { "Incomplete".into() },
            if individual {
                &[("keep", "Keep files"), ("delete", "Delete all"), ("finish", "Delete unfinished files")]
            } else {
                &[("keep", "Keep files"), ("delete", "Delete all"), ("finish", "Finish what's done")]
            },
            policy.incomplete,
            !known || n_i > 0,
            10,
            false,
            incomplete_text,
            cx,
        );

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
                        .w(px(640.))
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
                                .children(shown.into_iter().map(|(name, tag)| {
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap_2()
                                        .child(div().truncate().child(name))
                                        .when_some(tag, |d, t| {
                                            d.child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_xs()
                                                    .text_color(if t == "complete" {
                                                        theme::success()
                                                    } else {
                                                        theme::warning()
                                                    })
                                                    .child(t),
                                            )
                                        })
                                }))
                                .when(more > 0, |d| {
                                    d.child(
                                        div()
                                            .text_color(theme::text_muted())
                                            .child(format!("…and {more} more")),
                                    )
                                }),
                        )
                        .when(loading, |d| {
                            d.child(div().text_xs().text_color(theme::text_muted()).child("Checking which torrents are complete…"))
                        })
                        .child(complete_row)
                        .child(incomplete_row)
                        .when(fwd, |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .p_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_color(theme::text_muted())
                                    .child(format!(
                                        "Completion actions that will run on the finished files: {}. If an action fails the torrent is kept and marked \"needs attention\". Runs in the background; progress shows in the status and in Events.",
                                        if actions.is_empty() { "none configured (files stay where they are)".to_owned() } else { actions.clone() }
                                    )),
                            )
                        })
                        .when(legacy, |d| {
                            d.child(div().text_xs().text_color(theme::warning()).child(
                                "This server doesn't support remove policies: Keep removes the torrent only, anything else deletes all files.",
                            ))
                        })
                        .when(deletes, |d| {
                            d.child(div().text_xs().text_color(theme::error()).child(
                                "Files will be deleted from disk. This cannot be undone.",
                            ))
                        })
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
                                        if deletes { "Remove (deletes files)" } else { "Remove" },
                                    )
                                    .when(loading, |d| d.opacity(0.5))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if loading {
                                            return;
                                        }
                                        if let Some(d) = this.confirm_delete.take() {
                                            let ids = d.items.iter().map(|(id, _)| *id).collect();
                                            let action = if legacy {
                                                if d.policy.deletes_files(true, true) {
                                                    TorrentAction::Delete
                                                } else {
                                                    TorrentAction::Forget
                                                }
                                            } else {
                                                TorrentAction::Remove(d.policy)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_input.read(cx).text().to_owned();
        self.visible = list_state::visible_rows_in(
            &self.torrents,
            &query,
            self.filter,
            &self.category_filter,
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
            .children({
                #[cfg(not(target_family = "wasm"))]
                let bar = self.render_handler_prompt(cx);
                #[cfg(target_family = "wasm")]
                let bar: Option<gpui::AnyElement> = None;
                bar
            })
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
            .when_some(
                self.cleanup_panel.as_ref().map(|(p, _)| p.clone()),
                |d, p| d.child(widgets::modal("cleanup-overlay", 1000., p)),
            )
            .children(self.render_confirm(cx))
            .children(self.render_category_dialog(cx))
            .children(self.render_row_menu(window, cx));
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
        assert_eq!(host_port(&u("https://torrents.example.com/")), "torrents.example.com:443");
        assert_eq!(host_port(&u("http://192.0.2.2:3030/")), "192.0.2.2:3030");
        assert_eq!(host_port(&u("http://[::1]:3030/")), "[::1]:3030");
        assert_eq!(host_port(&u("http://example.com/")), "example.com:80");
    }

    #[test]
    fn native_connection_uses_socket_addrs() {
        let base = url::Url::parse("https://r.example/").unwrap();
        let c = api::ConnEndpoints {
            local: Some("192.0.2.30:54321".parse().unwrap()),
            remote: "[2001:db8::5]:443".parse().unwrap(),
        };
        let (text, tip) = connection_label(&base, Some(c));
        assert_eq!(text, "192.0.2.30:54321 <--> [2001:db8::5]:443");
        assert!(tip.contains("r.example:443"), "{tip}");
        let c = api::ConnEndpoints {
            local: None,
            remote: "192.0.2.101:3030".parse().unwrap(),
        };
        assert_eq!(connection_label(&base, Some(c)).0, "192.0.2.101:3030");
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
