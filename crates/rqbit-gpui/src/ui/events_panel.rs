//! Events view (web UI `EventsModal`): repair counters (with reset), the
//! event log filtered by type / severity / torrent, paging with "Load
//! older", and expandable rows with the raw record. Opening it marks
//! everything as seen (clears the header badge).

use std::collections::HashSet;
use std::time::Duration;

use gpui::{Context, EventEmitter, Task, Window, div, prelude::*, px};

use super::details::{format_event_time, kind_label, severity_color};
use super::{theme, widgets};
use crate::api::{ApiClient, EventQuery, EventRecord, EventSummary};
use crate::format::format_bytes;

const PAGE: u32 = 100;

/// (label, `kind` query value), as the web UI's `KIND_FILTERS`.
pub const KIND_FILTERS: &[(&str, &str)] = &[
    ("All types", ""),
    ("Repairs", "repair_run,repair_file"),
    ("Repair runs", "repair_run"),
    ("Errors", "io_error,torrent_error"),
    (
        "Recovery",
        "damage_detected,piece_retry_scheduled,needs_attention",
    ),
    ("Needs attention", "needs_attention"),
    ("Adoption", "adoption"),
];

/// (label, `severity` query value).
pub const SEVERITY_FILTERS: &[(&str, &str)] = &[
    ("All severities", ""),
    ("Warnings & errors", "warning"),
    ("Errors only", "error"),
];

pub enum EventsPanelEvent {
    Close,
    /// The newest seq shown (for the unseen badge).
    Seen(u64),
    /// A torrent link was clicked (info hash).
    OpenTorrent(String),
}

pub struct EventsPanel {
    client: ApiClient,
    kind: usize,
    severity: usize,
    /// Torrent filter: (info hash, label).
    torrent: Option<(String, String)>,
    /// Info hashes of torrents currently in the session (linkable).
    known: HashSet<String>,
    events: Vec<EventRecord>,
    loaded: bool,
    next_before: Option<u64>,
    loading: bool,
    error: Option<String>,
    summary: Option<EventSummary>,
    confirm_reset: bool,
    expanded: HashSet<u64>,
    /// Bumped on filter changes so stale responses are dropped.
    generation: u64,
    _poll: Task<()>,
}

impl EventEmitter<EventsPanelEvent> for EventsPanel {}

impl EventsPanel {
    pub fn new(
        client: ApiClient,
        torrent: Option<(String, String)>,
        known: HashSet<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let Ok(()) = this.update(cx, |p, cx| {
                    // Auto-refresh the first page (not while older pages are shown).
                    if p.events.len() <= PAGE as usize {
                        p.load(None, cx);
                    }
                    p.load_summary(cx);
                }) else {
                    return;
                };
                cx.background_executor()
                    .timer(Duration::from_secs(10))
                    .await;
            }
        });
        Self {
            client,
            kind: 0,
            severity: 0,
            torrent,
            known,
            events: Vec::new(),
            loaded: false,
            next_before: None,
            loading: false,
            error: None,
            summary: None,
            confirm_reset: false,
            expanded: HashSet::new(),
            generation: 0,
            _poll: poll,
        }
    }

    pub fn set_known(&mut self, known: HashSet<String>) {
        self.known = known;
    }

    fn query(&self, before: Option<u64>) -> EventQuery {
        let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        EventQuery {
            kind: nonempty(KIND_FILTERS[self.kind].1),
            severity: nonempty(SEVERITY_FILTERS[self.severity].1),
            info_hash: self.torrent.as_ref().map(|(h, _)| h.clone()),
            before_seq: before,
            limit: Some(PAGE),
            ..Default::default()
        }
    }

    fn load(&mut self, before: Option<u64>, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let q = self.query(before);
        let generation = self.generation;
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(client.get_events(&q)).await;
            this.update(cx, |p, cx| {
                if p.generation != generation {
                    return;
                }
                p.loading = false;
                match r {
                    Ok(page) => {
                        p.error = None;
                        if before.is_some() {
                            p.events.extend(page.events);
                        } else {
                            p.events = page.events;
                            cx.emit(EventsPanelEvent::Seen(page.latest_seq));
                        }
                        p.next_before = page.next_before_seq;
                        p.loaded = true;
                    }
                    Err(e) => p.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_summary(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.get_events_summary(None))
                .await;
            this.update(cx, |p, cx| {
                if let Ok(s) = r {
                    p.summary = Some(s);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.events.clear();
        self.next_before = None;
        self.loaded = false;
        self.expanded.clear();
        self.load(None, cx);
        cx.notify();
    }

    fn reset_counters(&mut self, cx: &mut Context<Self>) {
        self.confirm_reset = false;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.reset_event_counters())
                .await;
            this.update(cx, |p, cx| {
                if let Err(e) = r {
                    p.error = Some(format!("Reset failed: {e:#}"));
                }
                p.load_summary(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_counters(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let s = self.summary.as_ref()?;
        let c = &s.counters;
        let counter = |label: &'static str, value: String, bad: bool| {
            div()
                .flex()
                .flex_col()
                .min_w(px(88.))
                .child(div().text_xs().text_color(theme::text_muted()).child(label))
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .when(bad, |d| d.text_color(theme::error()))
                        .child(value),
                )
        };
        let since = chrono::DateTime::parse_from_rfc3339(&c.since)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|_| c.since.clone());
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(theme::border())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_end()
                        .gap_x_5()
                        .gap_y_2()
                        .child(counter("Auto repairs", c.auto_repairs.to_string(), false))
                        .child(counter(
                            "Manual repairs",
                            c.manual_repairs.to_string(),
                            false,
                        ))
                        .child(counter(
                            "Files repaired",
                            c.files_repaired.to_string(),
                            false,
                        ))
                        .child(counter("Zeroed", format_bytes(c.bytes_zeroed), false))
                        .child(counter(
                            "Re-download",
                            format_bytes(c.bytes_redownload),
                            false,
                        ))
                        .child(counter(
                            "Pieces requeued",
                            c.pieces_requeued.to_string(),
                            false,
                        ))
                        .child(counter("Give-ups", c.give_ups.to_string(), c.give_ups > 0))
                        .child(counter(
                            "Repair failures",
                            c.repair_failures.to_string(),
                            c.repair_failures > 0,
                        ))
                        .child(counter("I/O errors", c.io_errors.to_string(), false)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(format!(
                                    "Since {since} · log {} of {} ({} segment{})",
                                    format_bytes(s.log_bytes),
                                    format_bytes(s.log_cap_bytes),
                                    s.log_segments,
                                    if s.log_segments == 1 { "" } else { "s" }
                                )),
                        )
                        .child(div().flex_1())
                        .when(!self.confirm_reset, |d| {
                            d.child(
                                widgets::button("ev-reset", "Reset counters", true).on_click(
                                    cx.listener(|p, _, _, cx| {
                                        p.confirm_reset = true;
                                        cx.notify();
                                    }),
                                ),
                            )
                        })
                        .when(self.confirm_reset, |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .child("Reset repair counters? The event log itself is kept."),
                            )
                            .child(
                                widgets::danger_button("ev-reset-yes", "Reset")
                                    .on_click(cx.listener(|p, _, _, cx| p.reset_counters(cx))),
                            )
                            .child(
                                widgets::button("ev-reset-no", "Cancel", true).on_click(
                                    cx.listener(|p, _, _, cx| {
                                        p.confirm_reset = false;
                                        cx.notify();
                                    }),
                                ),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    fn render_row(&self, e: &EventRecord, cx: &mut Context<Self>) -> gpui::AnyElement {
        let seq = e.seq;
        let expanded = self.expanded.contains(&seq);
        let linkable = e
            .info_hash
            .as_ref()
            .filter(|h| self.known.contains(*h))
            .cloned();
        let name = e.torrent_name.clone().or_else(|| {
            e.info_hash
                .as_ref()
                .map(|h| format!("{}…", &h[..h.len().min(12)]))
        });
        let count = e.count.unwrap_or(1);
        let raw = serde_json::json!({
            "seq": e.seq,
            "time": e.time,
            "kind": e.kind,
            "torrent_id": e.torrent_id,
            "info_hash": e.info_hash,
            "file_id": e.file_id,
            "count": e.count,
            "details": e.details,
        });
        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .id(("ev-row", seq))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::surface_hover()))
                    .on_click(cx.listener(move |p, _, _, cx| {
                        if !p.expanded.remove(&seq) {
                            p.expanded.insert(seq);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(10.))
                            .flex_shrink_0()
                            .text_color(theme::text_muted())
                            .child(if expanded { "−" } else { "+" }),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .size(px(8.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(severity_color(&e.severity)),
                    )
                    .child(
                        div()
                            .w(px(96.))
                            .flex_shrink_0()
                            .text_color(theme::text_muted())
                            .child(format_event_time(&e.time)),
                    )
                    .child(
                        div()
                            .w(px(116.))
                            .flex_shrink_0()
                            .truncate()
                            .text_color(severity_color(&e.severity))
                            .child(kind_label(&e.kind)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .when_some(name, |d, name| {
                                d.child(match linkable {
                                    Some(hash) => widgets::link(("ev-open", seq), name)
                                        .text_sm()
                                        .truncate()
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.stop_propagation();
                                            cx.emit(EventsPanelEvent::OpenTorrent(hash.clone()))
                                        }))
                                        .into_any_element(),
                                    None => div()
                                        .truncate()
                                        .text_color(theme::text_muted())
                                        .child(name)
                                        .into_any_element(),
                                })
                            })
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(div().flex_1().min_w(px(0.)).child(e.message.clone()))
                                    .when(count > 1, |d| {
                                        d.child(
                                            div()
                                                .flex_shrink_0()
                                                .px_1()
                                                .rounded_sm()
                                                .bg(theme::error_bg())
                                                .text_xs()
                                                .text_color(theme::error())
                                                .child(format!("×{count}")),
                                        )
                                    }),
                            ),
                    ),
            )
            .when(expanded, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pl(px(40.))
                        .pr_2()
                        .pb_2()
                        .when_some(e.path.clone(), |d, p| {
                            d.child(div().text_xs().text_color(theme::text_muted()).child(p))
                        })
                        .child(
                            div()
                                .p_2()
                                .rounded_md()
                                .bg(theme::bg())
                                .text_xs()
                                .font_family("monospace")
                                .children(
                                    serde_json::to_string_pretty(&raw)
                                        .unwrap_or_default()
                                        .lines()
                                        .map(|l| {
                                            div()
                                                .whitespace_nowrap()
                                                .child(l.replace(' ', "\u{a0}"))
                                        })
                                        .collect::<Vec<_>>(),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
}

impl Render for EventsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Events: repairs & errors"),
            )
            .child(div().flex_1())
            .child(
                widgets::button("ev-close-x", "×", true)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(EventsPanelEvent::Close))),
            );

        let segments = |id: &'static str,
                        items: &'static [(&'static str, &'static str)],
                        current: usize,
                        set: fn(&mut EventsPanel, usize),
                        cx: &mut Context<Self>| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .children(items.iter().enumerate().map(|(i, (label, _))| {
                    widgets::segment((id, i), *label, current == i).on_click(cx.listener(
                        move |p, _, _, cx| {
                            set(p, i);
                            p.reload(cx);
                        },
                    ))
                }))
        };
        let filters = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(segments(
                "ev-kind",
                KIND_FILTERS,
                self.kind,
                |p, i| p.kind = i,
                cx,
            ))
            .child(segments(
                "ev-sev",
                SEVERITY_FILTERS,
                self.severity,
                |p, i| p.severity = i,
                cx,
            ))
            .when_some(self.torrent.clone(), |d, (_, label)| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::primary())
                        .text_sm()
                        .text_color(theme::primary())
                        .child(div().max_w(px(280.)).truncate().child(label))
                        .child(
                            div()
                                .id("ev-clear-torrent")
                                .cursor_pointer()
                                .child("×")
                                .on_click(cx.listener(|p, _, _, cx| {
                                    p.torrent = None;
                                    p.reload(cx);
                                })),
                        ),
                )
            })
            .child(div().flex_1())
            .child(
                widgets::button("ev-refresh", "Refresh", true)
                    .on_click(cx.listener(|p, _, _, cx| p.load(None, cx))),
            );

        let list = if self.events.is_empty() {
            div()
                .p_4()
                .text_sm()
                .text_color(theme::text_muted())
                .child(if self.loaded {
                    "No events."
                } else {
                    "Loading…"
                })
                .into_any_element()
        } else {
            let rows: Vec<_> = self.events.iter().map(|e| self.render_row(e, cx)).collect();
            div()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(theme::border())
                .children(rows)
                .into_any_element()
        };

        div().flex().flex_col().size_full().child(header).child(
            div()
                .id("ev-body")
                .flex()
                .flex_col()
                .gap_3()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .p_4()
                .children(self.render_counters(cx))
                .child(filters)
                .children(
                    self.error
                        .clone()
                        .map(|e| div().text_sm().text_color(theme::error()).child(e)),
                )
                .child(list)
                .when_some(self.next_before, |d, before| {
                    d.child(div().flex().justify_center().child(
                        widgets::button("ev-older", "Load older", !self.loading).when(
                            !self.loading,
                            |b| {
                                b.on_click(cx.listener(move |p, _, _, cx| p.load(Some(before), cx)))
                            },
                        ),
                    ))
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_match_web_ui() {
        assert_eq!(KIND_FILTERS.len(), 7);
        assert_eq!(KIND_FILTERS[0].1, "");
        assert!(
            KIND_FILTERS
                .iter()
                .any(|(_, v)| *v == "io_error,torrent_error")
        );
        assert_eq!(SEVERITY_FILTERS[2].1, "error");
    }
}
