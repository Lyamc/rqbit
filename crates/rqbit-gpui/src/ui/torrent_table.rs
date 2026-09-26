//! The torrent list: column layout, header and rows.
//!
//! Column order follows the web UI's compact table
//! (Name, Status, Size, Progress, ↓ Download, ↑ Upload) plus Ratio and the
//! per-row actions. Widths are shared by header and rows so they can't drift.

use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px, relative};

use super::{RqbitWindow, TorrentAction, theme, widgets};
use crate::api::{TorrentListItem, TorrentState};
use crate::format::{format_bytes, format_ratio, format_speed};

pub const ROW_HEIGHT: f32 = 34.;

const W_STATUS: f32 = 190.;
const W_SIZE: f32 = 84.;
const W_PROGRESS: f32 = 150.;
const W_SPEED: f32 = 96.;
const W_RATIO: f32 = 56.;
const W_ACTIONS: f32 = 210.;

fn cell(width: f32) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w(px(width))
        .flex_shrink_0()
        .px_2()
        .overflow_hidden()
        .whitespace_nowrap()
}

fn name_cell() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_1()
        .min_w(px(0.))
        .px_2()
        .overflow_hidden()
}

pub fn header() -> impl IntoElement {
    let label = |s: &'static str| div().truncate().child(s);
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .h(px(30.))
        .border_b_1()
        .border_color(theme::border())
        .text_xs()
        .text_color(theme::text_muted())
        .child(name_cell().child(label("Name")))
        .child(cell(W_STATUS).child(label("Status")))
        .child(cell(W_SIZE).flex().justify_end().child(label("Size")))
        .child(cell(W_PROGRESS).child(label("Progress")))
        .child(
            cell(W_SPEED)
                .flex()
                .justify_end()
                .child(label("↓ Download")),
        )
        .child(cell(W_SPEED).flex().justify_end().child(label("↑ Upload")))
        .child(cell(W_RATIO).flex().justify_end().child(label("Ratio")))
        .child(cell(W_ACTIONS).child(label("")))
}

pub fn row(
    t: &TorrentListItem,
    ix: usize,
    pending: bool,
    cx: &mut Context<RqbitWindow>,
) -> AnyElement {
    let id = t.id;
    let name: SharedString = t.display_name().into();
    let stats = t.stats.as_ref();

    let (status_label, status_kind) = match stats {
        Some(s) => (s.status_label(), s.status_kind().to_owned()),
        None => ("Unknown".to_owned(), "unknown".to_owned()),
    };
    let error: Option<SharedString> = stats.and_then(|s| s.error.clone()).map(Into::into);
    let size = stats
        .map(|s| format_bytes(s.total_bytes))
        .unwrap_or_default();
    let frac = stats.map(|s| s.progress_fraction()).unwrap_or(0.0);
    let live = stats.and_then(|s| s.live.as_ref());
    let speed = |sp: &crate::api::Speed| {
        sp.human_readable
            .clone()
            .unwrap_or_else(|| format_speed(sp.mbps))
    };
    let (down, up) = match live {
        Some(l) => (speed(&l.download_speed), speed(&l.upload_speed)),
        None => ("-".to_owned(), "-".to_owned()),
    };
    let ratio = stats
        .and_then(|s| s.session_ratio())
        .map(format_ratio)
        .unwrap_or_else(|| "-".into());
    let bar_color = match stats.map(|s| s.state) {
        Some(TorrentState::Error) => theme::error(),
        Some(TorrentState::Initializing) => theme::warning(),
        _ if stats.is_some_and(|s| s.finished) => theme::success(),
        _ => theme::primary(),
    };
    let can_start = !pending && stats.is_some_and(|s| s.can_start());
    let can_pause = !pending && stats.is_some_and(|s| s.can_pause());

    let status_tooltip: SharedString = match &error {
        Some(e) => format!("{status_label}: {e}").into(),
        None => status_label.clone().into(),
    };

    div()
        .id(("row", id))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .h(px(ROW_HEIGHT))
        .text_sm()
        .border_b_1()
        .border_color(theme::surface_hover())
        .when(ix % 2 == 1, |d| d.bg(theme::surface()))
        .hover(|s| s.bg(theme::surface_hover()))
        .child(
            name_cell().child(
                div()
                    .id(("name", id))
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(theme::text())
                    .tooltip(widgets::text_tooltip(name.clone()))
                    .child(name.clone()),
            ),
        )
        .child(
            cell(W_STATUS).child(
                div()
                    .id(("status", id))
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(theme::status_color(&status_kind))
                    .tooltip(widgets::text_tooltip(status_tooltip))
                    .child(match &error {
                        Some(e) => format!("{status_label} — {e}"),
                        None => status_label,
                    }),
            ),
        )
        .child(
            cell(W_SIZE)
                .flex()
                .justify_end()
                .text_color(theme::text_muted())
                .child(size),
        )
        .child(
            cell(W_PROGRESS)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .h(px(6.))
                        .rounded_full()
                        .bg(theme::border())
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .w(relative(frac as f32))
                                .bg(bar_color),
                        ),
                )
                .child(
                    div()
                        .w(px(36.))
                        .flex()
                        .justify_end()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(format!("{}%", (frac * 100.0).floor() as u32)),
                ),
        )
        .child(cell(W_SPEED).flex().justify_end().child(down))
        .child(cell(W_SPEED).flex().justify_end().child(up))
        .child(
            cell(W_RATIO)
                .flex()
                .justify_end()
                .text_color(theme::text_muted())
                .child(ratio),
        )
        .child(
            cell(W_ACTIONS)
                .flex()
                .flex_row()
                .gap_1()
                .justify_end()
                .child(
                    widgets::button(("start", id), "Start", can_start).when(can_start, |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| {
                            this.run_action(id, TorrentAction::Start, cx)
                        }))
                    }),
                )
                .child(
                    widgets::button(("pause", id), "Pause", can_pause).when(can_pause, |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| {
                            this.run_action(id, TorrentAction::Pause, cx)
                        }))
                    }),
                )
                .child(
                    widgets::button(("remove", id), "Remove", !pending).when(!pending, |b| {
                        let name = name.clone();
                        b.on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.ask_remove(id, name.clone(), cx)
                            }),
                        )
                    }),
                ),
        )
        .into_any_element()
}
