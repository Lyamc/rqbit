//! GPUI views.
//!
//! `RqbitWindow` is the root view: connection bar, status/error banners, the
//! torrent table and a confirm dialog. New web-UI features should be added as
//! their own modules/views here (e.g. `details.rs`, `add_torrent.rs`,
//! `events.rs`) with any new endpoints going into `crate::api`.

mod text_input;
mod theme;
mod torrent_table;
mod widgets;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px, uniform_list,
};

use crate::api::{ApiClient, ListTorrentsResponse, TorrentListItem, parse_base_url};
use crate::format::format_speed;
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
    http: reqwest::blocking::Client,
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
    _subscriptions: Vec<Subscription>,
}

impl RqbitWindow {
    pub fn new(
        http: reqwest::blocking::Client,
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
            http,
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
            _subscriptions: vec![sub],
        };
        this.connect(url, cx);
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
                let client = ApiClient::new(self.http.clone(), url);
                self.client = Some(client.clone());
                self.conn = ConnState::Connecting;
                let generation = self.generation;
                self.poll_task = Some(cx.spawn(async move |this, cx| {
                    loop {
                        let c = client.clone();
                        let res = cx
                            .background_executor()
                            .spawn(async move { c.list_torrents() })
                            .await;
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
            let c = client.clone();
            let res = cx
                .background_executor()
                .spawn(async move {
                    match action {
                        TorrentAction::Start => c.start(id),
                        TorrentAction::Pause => c.pause(id),
                        TorrentAction::Forget => c.forget(id),
                    }
                })
                .await;
            // Refresh immediately so the row reflects the new state.
            let list = cx
                .background_executor()
                .spawn(async move { client.list_torrents() })
                .await;
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
        div()
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
            .children(self.render_confirm(cx))
    }
}
