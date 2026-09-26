//! GPUI views.
//!
//! `RqbitWindow` is the root view: connection bar, status/error banners, the
//! torrent table and a confirm dialog. New web-UI features should be added as
//! their own modules/views here (e.g. `details.rs`, `add_torrent.rs`,
//! `events.rs`) with any new endpoints going into `crate::api`.

mod add_panel;
mod files;
mod prefs_panel;
mod text_input;
mod theme;
mod torrent_table;
mod widgets;

use std::collections::HashSet;
use std::time::Duration;

use crate::time::Instant;

use gpui::{
    Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px, uniform_list,
};

use crate::api::{ApiClient, ListTorrentsResponse, TorrentListItem, Transport, parse_base_url};
use crate::format::format_speed;
use add_panel::{AddPanel, AddPanelEvent};
use prefs_panel::{PrefsPanel, PrefsPanelEvent};
use text_input::{TextInput, TextInputEvent};

/// How often the torrent list is refreshed (the web UI polls about as often).
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Slower retry while the server is unreachable.
const RETRY_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug)]
pub enum TorrentAction {
    Start,
    Pause,
    Forget,
}

impl TorrentAction {
    fn verb(self) -> &'static str {
        match self {
            TorrentAction::Start => "Start",
            TorrentAction::Pause => "Pause",
            TorrentAction::Forget => "Remove",
        }
    }
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
    confirm_remove: Option<(usize, SharedString)>,
    poll_task: Option<Task<()>>,
    add_panel: Option<(Entity<AddPanel>, Subscription)>,
    prefs_panel: Option<(Entity<PrefsPanel>, Subscription)>,
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
            },
        );
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
            confirm_remove: None,
            poll_task: None,
            add_panel: None,
            prefs_panel: None,
            _subscriptions: vec![sub],
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
        self.confirm_remove = None;
        self.poll_task = None;
        self.client = None;

        match parse_base_url(&raw) {
            Err(e) => self.conn = ConnState::Invalid(format!("{e:#}")),
            Ok(url) => {
                let client = ApiClient::new(self.transport.clone(), url);
                self.client = Some(client.clone());
                self.conn = ConnState::Connecting;
                let generation = self.generation;
                self.poll_task = Some(cx.spawn(async move |this, cx| {
                    loop {
                        let res = cx.background_executor().spawn(client.list_torrents()).await;
                        let ok = res.is_ok();
                        let alive = this
                            .update(cx, |this, cx| {
                                if this.generation == generation {
                                    this.apply_list(res);
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
            }
        }
        cx.notify();
    }

    fn apply_list(&mut self, res: anyhow::Result<ListTorrentsResponse>) {
        match res {
            Ok(list) => {
                self.torrents = list.torrents;
                self.last_update = Some(Instant::now());
                self.conn = ConnState::Connected;
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

    pub(crate) fn run_action(&mut self, id: usize, action: TorrentAction, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if !self.pending.insert(id) {
            return;
        }
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let request = match action {
                TorrentAction::Start => client.start(id),
                TorrentAction::Pause => client.pause(id),
                TorrentAction::Forget => client.forget(id),
            };
            let res = cx.background_executor().spawn(request).await;
            // Refresh immediately so the row reflects the new state.
            let list = cx.background_executor().spawn(client.list_torrents()).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending.remove(&id);
                if let Err(e) = res {
                    this.action_error =
                        Some(format!("{} failed for torrent #{id}: {e:#}", action.verb()));
                }
                this.apply_list(list);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn ask_remove(&mut self, id: usize, name: SharedString, cx: &mut Context<Self>) {
        self.confirm_remove = Some((id, name));
        cx.notify();
    }

    /// Opens (or returns the open) Add panel.
    fn open_add(&mut self, cx: &mut Context<Self>) -> Option<Entity<AddPanel>> {
        if let Some((p, _)) = &self.add_panel {
            return Some(p.clone());
        }
        let client = self.client.clone()?;
        self.prefs_panel = None;
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
        let panel = cx.new(|cx| PrefsPanel::new(client, cx));
        let sub = cx.subscribe(&panel, |this, _, ev: &PrefsPanelEvent, cx| match ev {
            PrefsPanelEvent::Close => {
                this.prefs_panel = None;
                cx.notify();
            }
        });
        self.prefs_panel = Some((panel, sub));
        cx.notify();
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
                    this.apply_list(list);
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
        let (down, up) = self
            .torrents
            .iter()
            .filter_map(|t| t.stats.as_ref()?.live.as_ref())
            .fold((0.0, 0.0), |(d, u), l| {
                (d + l.download_speed.mbps, u + l.upload_speed.mbps)
            });
        let fmt = |v: f64| {
            let s = format_speed(v);
            if s.is_empty() {
                "0 Bytes/s".to_owned()
            } else {
                s
            }
        };
        let base = self
            .client
            .as_ref()
            .map(|c| c.base_url().to_string())
            .unwrap_or_default();
        div()
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
            .child(div().flex_1().truncate().child(base))
            .child(format!("↓ {}", fmt(down)))
            .child(format!("↑ {}", fmt(up)))
    }

    fn render_confirm(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (id, name) = self.confirm_remove.clone()?;
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
                        .w(px(460.))
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .text_color(theme::text())
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .child("Remove torrent?"),
                        )
                        .child(div().text_sm().child(name))
                        .child(div().text_sm().text_color(theme::text_muted()).child(
                            "The torrent is removed from rqbit. Downloaded files are kept on disk.",
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap_2()
                                .child(widgets::button("confirm-cancel", "Cancel", true).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.confirm_remove = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    widgets::danger_button("confirm-remove", "Remove").on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.confirm_remove = None;
                                            this.run_action(id, TorrentAction::Forget, cx);
                                        }),
                                    ),
                                ),
                        ),
                ),
        )
    }
}

impl Render for RqbitWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.torrents.len();
        let connected_empty = matches!(self.conn, ConnState::Connected) && count == 0;
        let root = div()
            .id("root")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::bg())
            .text_color(theme::text())
            .child(self.render_connection_bar(cx))
            .child(self.render_banners(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .px_3()
                    .pt_2()
                    .child(torrent_table::header())
                    .when(connected_empty, |d| {
                        d.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(theme::text_muted())
                                .child("No torrents."),
                        )
                    })
                    .child(
                        uniform_list(
                            "torrents",
                            count,
                            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                                range
                                    .filter_map(|ix| {
                                        let t = this.torrents.get(ix)?;
                                        let pending = this.pending.contains(&t.id);
                                        Some(torrent_table::row(t, ix, pending, cx))
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .flex_1(),
                    ),
            )
            .child(self.render_footer())
            .when_some(self.add_panel.as_ref().map(|(p, _)| p.clone()), |d, p| {
                d.child(widgets::modal("add-overlay", 820., p))
            })
            .when_some(self.prefs_panel.as_ref().map(|(p, _)| p.clone()), |d, p| {
                d.child(widgets::modal("prefs-overlay", 760., p))
            })
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
