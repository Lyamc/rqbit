//! The torrent list: column layout, header and rows.
//!
//! Columns follow the web UI's compact table (select, ID, queue #, Name,
//! Status, Category, Size, Progress, ↓, ↑, Received, Sent, ETA, Peers). Widths are
//! shared by header and rows so they can't drift. Click selects, Ctrl/Cmd+
//! click toggles, Shift+click extends, double click opens details; the
//! checkbox toggles. Clicking a header sorts (again: reverse).

use gpui::{AnyElement, ClickEvent, Context, Div, SharedString, div, prelude::*, px, relative};

use super::list_state::{SortColumn, SortDir};
use super::{RqbitWindow, theme, widgets};
use crate::api::{TorrentListItem, TorrentState};
use crate::format::format_bytes;

pub const ROW_HEIGHT: f32 = 32.;

const W_CHECK: f32 = 30.;
const W_ID: f32 = 44.;
const W_QUEUE: f32 = 36.;
const W_STATUS: f32 = 170.;
const W_CATEGORY: f32 = 120.;
const W_SIZE: f32 = 80.;
const W_PROGRESS: f32 = 130.;
const W_SPEED: f32 = 92.;
const W_BYTES: f32 = 80.;
const W_ETA: f32 = 70.;
const W_PEERS: f32 = 60.;

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
        .flex_col()
        .justify_center()
        .flex_1()
        .min_w(px(0.))
        .px_2()
        .overflow_hidden()
}

fn check_box(checked: bool, partial: bool) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(14.))
        .rounded_sm()
        .border_1()
        .border_color(if checked || partial {
            theme::primary()
        } else {
            theme::border()
        })
        .text_xs()
        .text_color(gpui::white())
        .when(checked, |d| d.bg(theme::primary()).child("✓"))
        .when(partial && !checked, |d| d.bg(theme::primary()).child("–"))
}

pub struct HeaderState {
    pub sort: SortColumn,
    pub dir: SortDir,
    pub all_selected: bool,
    pub some_selected: bool,
}

pub fn header(h: HeaderState, cx: &mut Context<RqbitWindow>) -> impl IntoElement {
    let sortable =
        |id: &'static str, label: &'static str, col: SortColumn, base: Div, right: bool| {
            let active = h.sort == col;
            let arrow = match (active, h.dir) {
                (false, _) => "",
                (true, SortDir::Asc) => " ↑",
                (true, SortDir::Desc) => " ↓",
            };
            base.id(id)
                .cursor_pointer()
                .hover(|s| s.text_color(theme::text()))
                .when(active, |d| d.text_color(theme::text()))
                .when(right, |d| d.justify_end())
                .on_click(cx.listener(move |this, _, _, cx| this.sort_by(col, cx)))
                .child(div().truncate().child(format!("{label}{arrow}")))
        };
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
        .child(
            cell(W_CHECK)
                .id("hdr-check")
                .justify_center()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.toggle_select_all(cx)))
                .child(check_box(h.all_selected, h.some_selected)),
        )
        .child(sortable("hdr-id", "ID", SortColumn::Id, cell(W_ID), false))
        .child(sortable(
            "hdr-queue",
            "#",
            SortColumn::Queue,
            cell(W_QUEUE),
            false,
        ))
        .child(sortable(
            "hdr-name",
            "Name",
            SortColumn::Name,
            name_cell().flex_row().justify_start(),
            false,
        ))
        .child(sortable(
            "hdr-status",
            "Status",
            SortColumn::Status,
            cell(W_STATUS),
            false,
        ))
        .child(sortable(
            "hdr-category",
            "Category",
            SortColumn::Category,
            cell(W_CATEGORY),
            false,
        ))
        .child(sortable(
            "hdr-size",
            "Size",
            SortColumn::Size,
            cell(W_SIZE),
            true,
        ))
        .child(sortable(
            "hdr-progress",
            "Progress",
            SortColumn::Progress,
            cell(W_PROGRESS),
            false,
        ))
        .child(sortable(
            "hdr-down",
            "↓ Download",
            SortColumn::Down,
            cell(W_SPEED),
            true,
        ))
        .child(sortable(
            "hdr-up",
            "↑ Upload",
            SortColumn::Up,
            cell(W_SPEED),
            true,
        ))
        .child(sortable(
            "hdr-recv",
            "Received",
            SortColumn::Received,
            cell(W_BYTES),
            true,
        ))
        .child(sortable(
            "hdr-sent",
            "Sent",
            SortColumn::Sent,
            cell(W_BYTES),
            true,
        ))
        .child(sortable(
            "hdr-eta",
            "ETA",
            SortColumn::Eta,
            cell(W_ETA),
            false,
        ))
        .child(sortable(
            "hdr-peers",
            "Peers",
            SortColumn::Peers,
            cell(W_PEERS),
            false,
        ))
}

/// Short damage / repair note under the name (web UI `damageShortText`).
pub fn damage_short_text(t: &TorrentListItem) -> Option<String> {
    let d = t.stats.as_ref()?.damage.as_ref()?;
    if let Some(r) = d.repair.as_ref().filter(|r| r.state == "running") {
        let pct = if r.total_bytes > 0 {
            r.scanned_bytes * 100 / r.total_bytes
        } else {
            0
        };
        return Some(format!(
            "Repairing damaged files… {pct}% scanned ({}/{} files)",
            r.files_done, r.files_total
        ));
    }
    let attention = if d.needs_attention == Some(true) {
        "Needs attention: "
    } else {
        ""
    };
    let rec = d.recovery.as_ref().and_then(|r| {
        let mut parts = Vec::new();
        if r.pieces_waiting > 0 {
            parts.push(format!(
                "{} piece(s) held back after I/O errors (attempt {}/{})",
                r.pieces_waiting, r.max_piece_attempts, r.max_attempts
            ));
        }
        if r.pieces_needing_attention > 0 {
            parts.push(format!(
                "{} piece(s) need attention",
                r.pieces_needing_attention
            ));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    });
    if !d.damaged_files.is_empty() {
        return Some(format!(
            "{attention}{} damaged file(s) (disk I/O errors) — use Fix errors → Repair{}",
            d.damaged_files.len(),
            rec.map(|r| format!(" ({r})")).unwrap_or_default()
        ));
    }
    if let Some(r) = rec {
        return Some(format!("{attention}{r} — Fix errors retries now"));
    }
    match d.repair.as_ref() {
        Some(r) if r.state == "failed" => Some(format!(
            "Repair failed: {}",
            r.error.as_deref().unwrap_or("unknown error")
        )),
        Some(r) if r.state == "done" => r.summary.as_ref().map(|s| {
            if s.files_repaired == 0 && s.files_failed == 0 {
                NO_DAMAGE_FOUND.to_owned()
            } else {
                let mut t = format!(
                    "Repaired: {} unreadable in {} file(s); {} piece(s) to re-download",
                    format_bytes(s.bytes_unreadable),
                    s.files_repaired + s.files_failed,
                    s.pieces_to_redownload
                );
                if s.files_failed > 0 {
                    t += &format!("; {} file(s) could not be repaired", s.files_failed);
                }
                t
            }
        }),
        _ => None,
    }
}

/// A repair that scanned the files and found nothing unreadable (not an alarm).
pub const NO_DAMAGE_FOUND: &str = "No damage found";

/// Colour for a [`damage_short_text`] line: muted for "No damage found", warning otherwise.
pub fn damage_text_color(text: &str) -> gpui::Rgba {
    if text == NO_DAMAGE_FOUND {
        theme::text_muted()
    } else {
        theme::warning()
    }
}

/// Per-row selection state.
#[derive(Clone, Copy, Default)]
pub struct RowState {
    pub selected: bool,
    /// Keyboard cursor (focus ring).
    pub focused: bool,
    pub pending: bool,
}

pub fn row(
    t: &TorrentListItem,
    ix: usize,
    state: RowState,
    cx: &mut Context<RqbitWindow>,
) -> AnyElement {
    let RowState {
        selected,
        focused,
        pending,
    } = state;
    let id = t.id;
    let name: SharedString = t.display_name().into();
    let stats = t.stats.as_ref();

    let (status_label, status_kind) = match stats {
        Some(s) => (s.status_label(), s.status_kind().to_owned()),
        None => ("Unknown".to_owned(), "unknown".to_owned()),
    };
    let error: Option<SharedString> = stats.and_then(|s| s.error.clone()).map(Into::into);
    let damage = damage_short_text(t);
    let category: SharedString = t.category_text().into();
    // Magnet still resolving metadata: the size isn't known yet.
    let size = stats
        .map(|s| {
            if s.total_bytes == 0 && !s.finished && t.total_pieces == 0 {
                "—".to_owned()
            } else {
                format_bytes(s.total_bytes)
            }
        })
        .unwrap_or_default();
    let frac = stats.map(|s| s.progress_fraction()).unwrap_or(0.0);
    let live = stats.and_then(|s| s.live.as_ref());
    let speed = |sp: &crate::api::Speed| sp.human_readable.clone().unwrap_or_else(|| "-".into());
    let (down, up) = match live {
        Some(l) => (speed(&l.download_speed), speed(&l.upload_speed)),
        None => ("-".to_owned(), "-".to_owned()),
    };
    let received = stats
        .map(|s| format_bytes(s.progress_bytes))
        .unwrap_or_default();
    let sent = live
        .map(|l| l.snapshot.uploaded_bytes)
        .filter(|b| *b > 0)
        .map(format_bytes)
        .unwrap_or_default();
    let eta = match stats {
        Some(s) if s.finished => "Done".to_owned(),
        Some(s) => s
            .live
            .as_ref()
            .and_then(|l| l.time_remaining.as_ref())
            .map(|t| t.human_readable.clone())
            .unwrap_or_else(|| "-".into()),
        None => "-".into(),
    };
    let peers = live
        .map(|l| {
            format!(
                "{}/{}",
                l.snapshot.peer_stats.live, l.snapshot.peer_stats.seen
            )
        })
        .unwrap_or_else(|| "-".into());
    let queue = stats
        .and_then(|s| s.queue_position)
        .map(|q| q.to_string())
        .unwrap_or_default();
    let bar_color = match stats.map(|s| s.state) {
        Some(TorrentState::Error) => theme::error(),
        Some(TorrentState::Initializing) => theme::warning(),
        _ if stats.is_some_and(|s| s.finished) => theme::success(),
        _ => theme::primary(),
    };
    let status_tooltip: SharedString = match &error {
        Some(e) => format!("{status_label}: {e}").into(),
        None => status_label.clone().into(),
    };
    let two_line = error.is_some() || damage.is_some();

    div()
        .id(("row", id))
        .relative()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .h(px(ROW_HEIGHT))
        .text_sm()
        .cursor_pointer()
        .border_b_1()
        .border_color(theme::surface_hover())
        .when(selected, |d| d.bg(theme::selected()))
        .when(!selected && ix % 2 == 1, |d| d.bg(theme::surface()))
        .when(!selected, |d| d.hover(|s| s.bg(theme::surface_hover())))
        .when(pending, |d| d.opacity(0.6))
        .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
            this.row_clicked(id, ev, window, cx)
        }))
        .on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                this.row_context_menu(id, ev.position, window, cx)
            }),
        )
        .child(
            cell(W_CHECK)
                .id(("check", id))
                .h_full()
                .justify_center()
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.toggle_selected(id, window, cx)
                }))
                .child(check_box(selected, false)),
        )
        .child(
            cell(W_ID)
                .text_color(theme::text_muted())
                .child(id.to_string()),
        )
        .child(cell(W_QUEUE).text_color(theme::text_muted()).child(queue))
        .child(
            name_cell()
                .child(
                    div()
                        .id(("name", id))
                        .w_full()
                        .truncate()
                        .text_color(theme::text())
                        .when(two_line, |d| d.text_xs())
                        .tooltip(widgets::text_tooltip(name.clone()))
                        .child(name.clone()),
                )
                .when_some(error.clone(), |d, e| {
                    d.child(
                        div()
                            .w_full()
                            .truncate()
                            .text_xs()
                            .text_color(theme::error())
                            .child(e),
                    )
                })
                .when_some(damage.filter(|_| error.is_none()), |d, e| {
                    d.child(
                        div()
                            .w_full()
                            .truncate()
                            .text_xs()
                            .text_color(damage_text_color(&e))
                            .child(e),
                    )
                }),
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
                    .child(status_label),
            ),
        )
        .child(
            cell(W_CATEGORY).child(
                div()
                    .id(("category", id))
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(theme::text_muted())
                    .when(!category.is_empty(), |d| {
                        d.tooltip(widgets::text_tooltip(category.clone()))
                    })
                    .child(category),
            ),
        )
        .child(
            cell(W_SIZE)
                .justify_end()
                .text_color(theme::text_muted())
                .child(size),
        )
        .child(
            cell(W_PROGRESS)
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
                        .w(px(34.))
                        .flex()
                        .justify_end()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(format!("{}%", progress_label(stats, 0))),
                ),
        )
        .child(cell(W_SPEED).justify_end().child(down))
        .child(cell(W_SPEED).justify_end().child(up))
        .child(
            cell(W_BYTES)
                .justify_end()
                .text_color(theme::text_muted())
                .child(received),
        )
        .child(
            cell(W_BYTES)
                .justify_end()
                .text_color(theme::text_muted())
                .child(sent),
        )
        .child(cell(W_ETA).text_color(theme::text_muted()).child(eta))
        .child(cell(W_PEERS).text_color(theme::text_muted()).child(peers))
        // Keyboard cursor: an outline drawn over the row (no layout shift).
        .when(focused, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .border_1()
                    .border_color(theme::primary()),
            )
        })
        .into_any_element()
}

/// Floored progress text for a torrent row (no stats yet / no metadata = 0).
fn progress_label(stats: Option<&crate::api::TorrentStats>, decimals: u32) -> String {
    match stats {
        Some(s) if s.total_bytes == 0 && !s.finished => crate::format::format_progress(0, 1, decimals),
        Some(s) => crate::format::format_progress(s.progress_bytes, s.total_bytes, decimals),
        None => crate::format::format_progress(0, 1, decimals),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{DamageStats, RepairStatus, RepairSummary, TorrentStats};

    fn with_repair(summary: RepairSummary) -> TorrentListItem {
        TorrentListItem {
            stats: Some(TorrentStats {
                damage: Some(DamageStats {
                    repair: Some(RepairStatus {
                        state: "done".into(),
                        auto: true,
                        summary: Some(summary),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn repair_that_found_nothing_is_not_reported_as_repaired() {
        let t = with_repair(RepairSummary {
            files_scanned: 3,
            ..Default::default()
        });
        assert_eq!(damage_short_text(&t).as_deref(), Some(NO_DAMAGE_FOUND));

        let t = with_repair(RepairSummary {
            files_scanned: 1,
            files_repaired: 1,
            bytes_unreadable: 4096,
            bytes_zeroed: 4096,
            pieces_to_redownload: 1,
            ..Default::default()
        });
        let text = damage_short_text(&t).unwrap();
        assert!(text.starts_with("Repaired: "), "{text}");
        assert!(text.contains("1 piece(s) to re-download"), "{text}");
    }
}
