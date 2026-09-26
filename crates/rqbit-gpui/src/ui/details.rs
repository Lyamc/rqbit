//! Torrent details pane (web UI compact `DetailPane`): Overview, Files,
//! Peers and Events tabs for the single selected torrent.
//!
//! - Overview: status / queue position, pieces bar (`/haves`), progress,
//!   speeds, peers, hash, output folder, playlist URL, damaged files and
//!   repair status (+ Repair), repair count, completion hooks (from
//!   preferences), actions (Resume / Pause / Force recheck / Fix errors /
//!   Delete) and, under "Move", relocate to another folder.
//! - Files: include/exclude per file (`update_only_files`), per-file
//!   progress, rename (`rename_file`).
//! - Peers: live peers with client, connection kind, totals and speeds
//!   (computed between polls, as the web UI does).
//! - Events: this torrent's recent events.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::{Context, Entity, EventEmitter, Task, Window, div, prelude::*, px, relative};
use serde_json::Value;

use super::text_input::TextInput;
use super::torrent_table::damage_short_text;
use super::{TorrentAction, theme, widgets};
use crate::api::{
    ApiClient, EventQuery, EventRecord, PeerStatsSnapshot, TorrentDetails, TorrentListItem,
    TorrentState,
};
use crate::format::format_bytes;
use crate::time::Instant;

pub enum DetailsEvent {
    Action(usize, TorrentAction),
    Close,
    Changed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Overview,
    Files,
    Peers,
    Events,
}

pub struct DetailsPanel {
    client: ApiClient,
    id: usize,
    torrent: Option<TorrentListItem>,
    tab: Tab,
    details: Option<TorrentDetails>,
    haves: Option<Vec<u8>>,
    peers: Option<PeerStatsSnapshot>,
    /// Previous peer byte counters, for speeds.
    peer_prev: HashMap<String, (u64, u64)>,
    peer_speeds: HashMap<String, (f64, f64)>,
    peer_prev_at: Option<Instant>,
    events: Option<Vec<EventRecord>>,
    prefs: Option<serde_json::Map<String, Value>>,
    /// File ids with an include/exclude change in flight.
    saving_files: bool,
    rename_file: Option<usize>,
    rename_input: Entity<TextInput>,
    move_open: bool,
    move_input: Entity<TextInput>,
    move_copy: bool,
    busy: bool,
    message: Option<String>,
    error: Option<String>,
    _poll: Task<()>,
}

impl EventEmitter<DetailsEvent> for DetailsPanel {}

fn has_piece(bits: &[u8], i: usize) -> bool {
    bits.get(i / 8)
        .is_some_and(|b| (b >> (7 - (i % 8))) & 1 == 1)
}

/// Fraction of pieces present in each of `buckets` equal slices.
pub fn piece_buckets(bits: &[u8], total: usize, buckets: usize) -> Vec<f32> {
    if total == 0 || buckets == 0 {
        return Vec::new();
    }
    let buckets = buckets.min(total);
    (0..buckets)
        .map(|b| {
            let start = b * total / buckets;
            let end = ((b + 1) * total / buckets).max(start + 1);
            let have = (start..end).filter(|i| has_piece(bits, *i)).count();
            have as f32 / (end - start) as f32
        })
        .collect()
}

fn count_pieces(bits: &[u8], total: usize) -> usize {
    (0..total).filter(|i| has_piece(bits, *i)).count()
}

fn fmt_speed_bps(bps: f64) -> String {
    if bps < 1.0 {
        "-".into()
    } else {
        format!("{}/s", format_bytes(bps as u64))
    }
}

impl DetailsPanel {
    pub fn new(client: ApiClient, id: usize, cx: &mut Context<Self>) -> Self {
        let rename_input = cx.new(|cx| TextInput::new("", "new/relative/path.ext", cx));
        let move_input = cx.new(|cx| TextInput::new("", "/destination/folder", cx));
        let poll = cx.spawn(async move |this, cx| {
            let mut n: u64 = 0;
            loop {
                let Ok(()) = this.update(cx, |p, cx| p.poll(n, cx)) else {
                    return;
                };
                n += 1;
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        let mut this = Self {
            client,
            id,
            torrent: None,
            tab: Tab::Overview,
            details: None,
            haves: None,
            peers: None,
            peer_prev: HashMap::new(),
            peer_speeds: HashMap::new(),
            peer_prev_at: None,
            events: None,
            prefs: None,
            saving_files: false,
            rename_file: None,
            rename_input,
            move_open: false,
            move_input,
            move_copy: false,
            busy: false,
            message: None,
            error: None,
            _poll: poll,
        };
        this.load_prefs(cx);
        this.load_events(cx);
        this
    }

    pub fn id(&self) -> usize {
        self.id
    }

    /// Latest list entry (pushed by the window on every render).
    pub fn set_torrent(&mut self, t: Option<TorrentListItem>) {
        self.torrent = t;
    }

    fn active(&self) -> bool {
        self.torrent
            .as_ref()
            .and_then(|t| t.stats.as_ref())
            .is_some_and(|s| {
                matches!(s.state, TorrentState::Live | TorrentState::Initializing) && !s.finished
            })
    }

    fn poll(&mut self, n: u64, cx: &mut Context<Self>) {
        // Haves: every 2 s while downloading, else every 30 s (web UI).
        if n == 0 || self.active() || n.is_multiple_of(15) {
            self.load_haves(cx);
        }
        match self.tab {
            Tab::Peers => self.load_peers(cx),
            Tab::Files if self.details.is_none() => self.load_details(cx),
            Tab::Events if n.is_multiple_of(5) => self.load_events(cx),
            Tab::Overview if n.is_multiple_of(15) => self.load_events(cx),
            _ => {}
        }
    }

    fn load_haves(&mut self, cx: &mut Context<Self>) {
        let (client, id) = (self.client.clone(), self.id);
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(client.get_haves(id)).await;
            this.update(cx, |p, cx| {
                if let Ok(b) = r {
                    p.haves = Some(b);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_details(&mut self, cx: &mut Context<Self>) {
        let (client, id) = (self.client.clone(), self.id);
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(client.get_details(id)).await;
            this.update(cx, |p, cx| {
                match r {
                    Ok(d) => p.details = Some(d),
                    Err(e) => p.error = Some(format!("Error loading files: {e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_peers(&mut self, cx: &mut Context<Self>) {
        let (client, id) = (self.client.clone(), self.id);
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.get_peer_stats(id))
                .await;
            this.update(cx, |p, cx| {
                if let Ok(snap) = r {
                    let now = Instant::now();
                    let dt = p
                        .peer_prev_at
                        .map(|t| now.duration_since(t).as_secs_f64())
                        .filter(|d| *d > 0.2);
                    let mut speeds = HashMap::new();
                    let mut prev = HashMap::new();
                    for (addr, ps) in &snap.peers {
                        let cur = (ps.counters.fetched_bytes, ps.counters.uploaded_bytes);
                        if let (Some(dt), Some(old)) = (dt, p.peer_prev.get(addr)) {
                            speeds.insert(
                                addr.clone(),
                                (
                                    cur.0.saturating_sub(old.0) as f64 / dt,
                                    cur.1.saturating_sub(old.1) as f64 / dt,
                                ),
                            );
                        }
                        prev.insert(addr.clone(), cur);
                    }
                    p.peer_prev = prev;
                    p.peer_speeds = speeds;
                    p.peer_prev_at = Some(now);
                    p.peers = Some(snap);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_events(&mut self, cx: &mut Context<Self>) {
        let (client, id) = (self.client.clone(), self.id);
        let info_hash = self.torrent.as_ref().map(|t| t.info_hash.clone());
        cx.spawn(async move |this, cx| {
            // Ids can change across restarts; the web UI matches by hash.
            let q = EventQuery {
                torrent_id: if info_hash.is_none() { Some(id) } else { None },
                info_hash,
                limit: Some(50),
                ..Default::default()
            };
            let r = cx.background_executor().spawn(client.get_events(&q)).await;
            this.update(cx, |p, cx| {
                if let Ok(page) = r {
                    p.events = Some(page.events);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_prefs(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.get_preferences())
                .await;
            this.update(cx, |p, cx| {
                if let Ok(prefs) = r {
                    p.prefs = Some(prefs);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        match tab {
            Tab::Files => self.load_details(cx),
            Tab::Peers => self.load_peers(cx),
            Tab::Events => self.load_events(cx),
            Tab::Overview => {}
        }
        cx.notify();
    }

    /// Runs one API call, then reports success/failure in the pane.
    fn run(
        &mut self,
        label: &'static str,
        fut: crate::api::ApiFuture<()>,
        reload_details: bool,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        self.error = None;
        self.message = None;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(fut).await;
            this.update(cx, |p, cx| {
                p.busy = false;
                p.saving_files = false;
                match r {
                    Ok(()) => {
                        p.message = Some(format!("{label}: done."));
                        cx.emit(DetailsEvent::Changed);
                    }
                    Err(e) => p.error = Some(format!("{label} failed: {e:#}")),
                }
                if reload_details {
                    p.load_details(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn toggle_file(&mut self, file_id: usize, cx: &mut Context<Self>) {
        let Some(d) = &mut self.details else { return };
        let mut included: HashSet<usize> = d
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.included)
            .map(|(i, _)| i)
            .collect();
        if !included.remove(&file_id) {
            included.insert(file_id);
        }
        self.set_included(included, cx);
    }

    fn set_included(&mut self, included: HashSet<usize>, cx: &mut Context<Self>) {
        let Some(d) = &mut self.details else { return };
        if included.is_empty() {
            self.error = Some("At least one file must stay selected.".into());
            cx.notify();
            return;
        }
        // Optimistic update.
        for (i, f) in d.files.iter_mut().enumerate() {
            f.included = included.contains(&i);
        }
        let mut ids: Vec<usize> = included.into_iter().collect();
        ids.sort_unstable();
        self.saving_files = true;
        let fut = self.client.update_only_files(self.id, &ids);
        self.run("Update selected files", fut, true, cx);
    }

    fn render_overview(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(t) = self.torrent.clone() else {
            return div().child("Loading…").into_any_element();
        };
        let Some(s) = t.stats.clone() else {
            return div().child("Loading…").into_any_element();
        };
        let lv = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_row()
                .gap_1()
                .child(div().text_color(theme::text_muted()).child(label))
                .child(value)
        };
        let total_pieces = t.total_pieces as usize;
        let buckets = self
            .haves
            .as_ref()
            .map(|h| piece_buckets(h, total_pieces, 300))
            .unwrap_or_default();
        let have_pieces = self.haves.as_ref().map(|h| count_pieces(h, total_pieces));
        let live = s.live.as_ref();
        let speed = |sp: Option<&crate::api::Speed>| {
            sp.and_then(|s| s.human_readable.clone())
                .unwrap_or_else(|| "-".into())
        };
        let eta = if s.finished {
            "Complete".to_owned()
        } else {
            live.and_then(|l| l.time_remaining.as_ref())
                .map(|t| t.human_readable.clone())
                .unwrap_or_else(|| "-".into())
        };
        let id = self.id;
        let can_start = s.state == TorrentState::Paused || s.state == TorrentState::Error;
        let can_pause = s.state == TorrentState::Live;
        let fix = super::list_state::fix_action(Some(&s));
        let playlist = self
            .client
            .base_url()
            .join(&format!("torrents/{id}/playlist"))
            .map(|u| u.to_string())
            .unwrap_or_default();
        let hooks = self.prefs.as_ref().map(|p| {
            let mut out = Vec::new();
            let actions = p
                .get("completion_actions")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            if actions > 0 {
                out.push(format!("{actions} completion action(s) configured"));
            } else {
                if let Some(h) = p
                    .get("on_complete_hook")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    out.push(format!("on-complete hook: {h}"));
                }
                if let Some(m) = p
                    .get("move_completed_path")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    let copy = p.get("move_completed_copy").and_then(Value::as_bool) == Some(true);
                    out.push(format!(
                        "{} completed to {m}",
                        if copy { "copy" } else { "move" }
                    ));
                }
                if p.get("auto_organize_enabled").and_then(Value::as_bool) == Some(true) {
                    out.push("auto-organize".into());
                }
            }
            if out.is_empty() {
                "none".to_owned()
            } else {
                out.join(" · ")
            }
        });
        let damage = s.damage.clone();
        let recent: Vec<EventRecord> = self
            .events
            .as_ref()
            .map(|e| e.iter().take(5).cloned().collect())
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t.display_name()),
                    )
                    .when_some(s.queue_position, |d, q| {
                        d.child(
                            div()
                                .text_color(theme::text_muted())
                                .child(format!("Queue #{q}")),
                        )
                    })
                    .child(
                        div()
                            .text_color(theme::status_color(s.status_kind()))
                            .child(s.status_label()),
                    ),
            )
            .when(!buckets.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .w_full()
                        .h(px(10.))
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(theme::border())
                        .children(buckets.iter().map(|f| {
                            let mut c = theme::success();
                            c.a = if *f <= 0.0 { 0.0 } else { 0.35 + 0.65 * f };
                            div().flex_1().h_full().bg(c)
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_x_4()
                    .child(lv(
                        "Progress",
                        format!(
                            "{:.1}% ({}/{})",
                            s.progress_fraction() * 100.0,
                            format_bytes(s.progress_bytes),
                            format_bytes(s.total_bytes)
                        ),
                    ))
                    .child(lv("Down", speed(live.map(|l| &l.download_speed))))
                    .child(lv(
                        "Up",
                        format!(
                            "{} ({} total)",
                            speed(live.map(|l| &l.upload_speed)),
                            format_bytes(live.map(|l| l.snapshot.uploaded_bytes).unwrap_or(0))
                        ),
                    ))
                    .child(lv("ETA", eta)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_x_4()
                    .when(total_pieces > 0, |d| {
                        d.child(lv(
                            "Pieces",
                            format!(
                                "{}/{} ({} each)",
                                have_pieces
                                    .map(|h| h.to_string())
                                    .unwrap_or_else(|| "?".into()),
                                total_pieces,
                                format_bytes(s.total_bytes / total_pieces.max(1) as u64)
                            ),
                        ))
                    })
                    .when_some(live.map(|l| l.snapshot.peer_stats.clone()), |d, p| {
                        d.child(lv(
                            "Peers",
                            format!(
                                "{} live, {} connecting, {} queued, {} seen, {} dead",
                                p.live, p.connecting, p.queued, p.seen, p.dead
                            ),
                        ))
                    })
                    .child(lv("Repairs", s.repair_count.unwrap_or(0).to_string())),
            )
            .child(lv("Hash", t.info_hash.clone()))
            .child(lv("Output", t.output_folder.clone()))
            .child(lv("Playlist", playlist))
            .when_some(hooks, |d, h| d.child(lv("On completion", h)))
            .when_some(s.error.clone(), |d, e| {
                d.child(div().text_color(theme::error()).child(e))
            })
            .when_some(damage_short_text(&t), |d, e| {
                d.child(div().text_color(theme::warning()).child(e))
            })
            .when_some(damage.filter(|d| !d.damaged_files.is_empty()), |d, dmg| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::warning())
                        .children(dmg.damaged_files.iter().take(8).map(|f| {
                            div().text_xs().truncate().child(format!(
                                "{} — {} error(s){}, {} piece(s): {}",
                                f.path,
                                f.errors,
                                if f.eio { " (EIO)" } else { "" },
                                f.pieces_failed,
                                f.last_error
                            ))
                        }))
                        .child(
                            div().flex().flex_row().child(
                                widgets::button(
                                    "repair-files",
                                    "Repair damaged files",
                                    !self.busy && !dmg.is_repair_running(),
                                )
                                .when(
                                    !self.busy && !dmg.is_repair_running(),
                                    |b| {
                                        b.on_click(cx.listener(move |p, _, _, cx| {
                                            let fut = p.client.repair_files(id, None);
                                            p.run("Repair", fut, false, cx);
                                        }))
                                    },
                                ),
                            ),
                        ),
                )
            })
            .when(!recent.is_empty(), |d| {
                d.child(
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("Recent events"),
                )
                .children(recent.into_iter().map(render_event))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_1()
                    .pt_1()
                    .child(
                        widgets::button("d-start", "Resume", can_start).when(can_start, |b| {
                            b.on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(DetailsEvent::Action(id, TorrentAction::Start))
                            }))
                        }),
                    )
                    .child(
                        widgets::button("d-pause", "Pause", can_pause).when(can_pause, |b| {
                            b.on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(DetailsEvent::Action(id, TorrentAction::Pause))
                            }))
                        }),
                    )
                    .child(
                        widgets::button("d-recheck", "Force recheck", true)
                            .tooltip(widgets::text_tooltip("Re-hash every piece on disk".into()))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(DetailsEvent::Action(id, TorrentAction::Recheck))
                            })),
                    )
                    .child(
                        widgets::button(
                            "d-fix",
                            if fix == super::list_state::FixAction::Repair {
                                "Repair"
                            } else {
                                "Fix errors"
                            },
                            fix != super::list_state::FixAction::Skip,
                        )
                        .when(
                            fix != super::list_state::FixAction::Skip,
                            |b| {
                                b.on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(DetailsEvent::Action(id, TorrentAction::FixErrors))
                                }))
                            },
                        ),
                    )
                    .child(
                        widgets::button(
                            "d-move",
                            if self.move_open {
                                "Move −"
                            } else {
                                "Move…"
                            },
                            true,
                        )
                        .on_click(cx.listener(|p, _, _, cx| {
                            p.move_open = !p.move_open;
                            if p.move_open {
                                let folder = p
                                    .torrent
                                    .as_ref()
                                    .map(|t| t.output_folder.clone())
                                    .unwrap_or_default();
                                p.move_input.update(cx, |i, cx| i.set_text(folder, cx));
                            }
                            cx.notify();
                        })),
                    ),
            )
            .when(self.move_open, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(380.)).child(self.move_input.clone()))
                        .child(
                            widgets::checkbox("move-copy", "Copy", self.move_copy, true).on_click(
                                cx.listener(|p, _, _, cx| {
                                    p.move_copy = !p.move_copy;
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            widgets::primary_button("move-go", "Move files", !self.busy).when(
                                !self.busy,
                                |b| {
                                    b.on_click(cx.listener(move |p, _, _, cx| {
                                        let dest = p.move_input.read(cx).text().trim().to_owned();
                                        if dest.is_empty() {
                                            return;
                                        }
                                        let fut = p.client.relocate(id, &dest, p.move_copy);
                                        p.run(
                                            if p.move_copy { "Copy" } else { "Move" },
                                            fut,
                                            false,
                                            cx,
                                        );
                                    }))
                                },
                            ),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_files(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(d) = self.details.clone() else {
            return div()
                .text_sm()
                .text_color(theme::text_muted())
                .child("Loading…")
                .into_any_element();
        };
        let progress = self
            .torrent
            .as_ref()
            .and_then(|t| t.stats.as_ref())
            .map(|s| s.file_progress.clone())
            .unwrap_or_default();
        let editable = !self.saving_files;
        let n = d.files.len();
        let all: HashSet<usize> = (0..n).collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .items_center()
                    .child(widgets::button("files-all", "Select all", editable).when(
                        editable,
                        |b| {
                            b.on_click(
                                cx.listener(move |p, _, _, cx| p.set_included(all.clone(), cx)),
                            )
                        },
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(format!(
                                "{n} file(s), {} selected",
                                d.files.iter().filter(|f| f.included).count()
                            )),
                    )
                    .when(self.saving_files, |d| {
                        d.child(div().text_xs().child("Saving…"))
                    }),
            )
            .when_some(self.rename_file, |el, fid| {
                el.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().text_xs().child(format!("Rename file #{fid} to")))
                        .child(div().w(px(420.)).child(self.rename_input.clone()))
                        .child(
                            widgets::primary_button("rename-go", "Rename", !self.busy).when(
                                !self.busy,
                                |b| {
                                    b.on_click(cx.listener(move |p, _, _, cx| {
                                        let new_path =
                                            p.rename_input.read(cx).text().trim().to_owned();
                                        if new_path.is_empty() {
                                            return;
                                        }
                                        p.rename_file = None;
                                        let fut = p.client.rename_file(p.id, fid, &new_path);
                                        p.run("Rename", fut, true, cx);
                                    }))
                                },
                            ),
                        )
                        .child(widgets::button("rename-cancel", "Cancel", true).on_click(
                            cx.listener(|p, _, _, cx| {
                                p.rename_file = None;
                                cx.notify();
                            }),
                        )),
                )
            })
            .children(
                d.files
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| !f.attributes.padding)
                    .map(|(i, f)| {
                        let done = progress.get(i).copied().unwrap_or(0);
                        let frac = if f.length > 0 {
                            done as f32 / f.length as f32
                        } else {
                            1.0
                        };
                        let name = f.name.clone();
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .h(px(24.))
                            .child(
                                widgets::checkbox(("file-inc", i), "", f.included, editable).when(
                                    editable,
                                    |b| {
                                        b.on_click(
                                            cx.listener(move |p, _, _, cx| p.toggle_file(i, cx)),
                                        )
                                    },
                                ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .child(f.name.clone()),
                            )
                            .child(
                                div()
                                    .w(px(80.))
                                    .flex()
                                    .justify_end()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(format_bytes(f.length)),
                            )
                            .child(
                                div()
                                    .w(px(90.))
                                    .h(px(5.))
                                    .rounded_full()
                                    .bg(theme::border())
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .w(relative(frac.clamp(0.0, 1.0)))
                                            .bg(if frac >= 1.0 {
                                                theme::success()
                                            } else {
                                                theme::primary()
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(40.))
                                    .flex()
                                    .justify_end()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(format!("{}%", (frac * 100.0).floor() as u32)),
                            )
                            .child(widgets::link(("file-rename", i), "rename").on_click(
                                cx.listener(move |p, _, _, cx| {
                                    p.rename_file = Some(i);
                                    let n = name.clone();
                                    p.rename_input.update(cx, |inp, cx| inp.set_text(n, cx));
                                    cx.notify();
                                }),
                            ))
                    }),
            )
            .into_any_element()
    }

    fn render_peers(&self) -> gpui::AnyElement {
        let Some(snap) = &self.peers else {
            return div()
                .text_sm()
                .text_color(theme::text_muted())
                .child("Loading…")
                .into_any_element();
        };
        if snap.peers.is_empty() {
            return div()
                .text_sm()
                .text_color(theme::text_muted())
                .child("No live peers.")
                .into_any_element();
        }
        let mut peers: Vec<_> = snap.peers.iter().collect();
        peers.sort_by(|a, b| {
            let sa = self.peer_speeds.get(a.0).map(|s| s.0).unwrap_or(0.0);
            let sb = self.peer_speeds.get(b.0).map(|s| s.0).unwrap_or(0.0);
            sb.partial_cmp(&sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1.counters.fetched_bytes.cmp(&a.1.counters.fetched_bytes))
        });
        let head = |w: f32, t: &'static str| div().w(px(w)).flex_shrink_0().child(t);
        div()
            .flex()
            .flex_col()
            .text_xs()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .pb_1()
                    .text_color(theme::text_muted())
                    .child(div().flex_1().child("Address"))
                    .child(head(150., "Client"))
                    .child(head(50., "Conn"))
                    .child(head(80., "↓ Speed"))
                    .child(head(80., "↑ Speed"))
                    .child(head(80., "Received"))
                    .child(head(80., "Sent"))
                    .child(head(60., "Pieces")),
            )
            .children(peers.into_iter().map(|(addr, p)| {
                let (d, u) = self.peer_speeds.get(addr).copied().unwrap_or((0.0, 0.0));
                let cell = |w: f32, t: String| div().w(px(w)).flex_shrink_0().truncate().child(t);
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .h(px(20.))
                    .items_center()
                    .child(div().flex_1().min_w(px(0.)).truncate().child(addr.clone()))
                    .child(cell(150., p.client_name.clone().unwrap_or_default()))
                    .child(cell(50., p.conn_kind.clone().unwrap_or_default()))
                    .child(cell(80., fmt_speed_bps(d)))
                    .child(cell(80., fmt_speed_bps(u)))
                    .child(cell(80., format_bytes(p.counters.fetched_bytes)))
                    .child(cell(80., format_bytes(p.counters.uploaded_bytes)))
                    .child(cell(
                        60.,
                        p.counters.downloaded_and_checked_pieces.to_string(),
                    ))
            }))
            .into_any_element()
    }

    fn render_events(&self) -> gpui::AnyElement {
        match &self.events {
            None => div()
                .text_sm()
                .text_color(theme::text_muted())
                .child("Loading…")
                .into_any_element(),
            Some(e) if e.is_empty() => div()
                .text_sm()
                .text_color(theme::text_muted())
                .child("No events for this torrent.")
                .into_any_element(),
            Some(e) => div()
                .flex()
                .flex_col()
                .gap_1()
                .children(e.iter().cloned().map(render_event))
                .into_any_element(),
        }
    }
}

pub fn severity_color(sev: &str) -> gpui::Rgba {
    match sev {
        "error" => theme::error(),
        "warning" => theme::warning(),
        _ => theme::text_muted(),
    }
}

pub fn kind_label(kind: &str) -> String {
    match kind {
        "repair_run" => "Repair",
        "repair_file" => "File repair",
        "piece_retry_scheduled" => "Retry",
        "needs_attention" => "Needs attention",
        "io_error" => "I/O error",
        "damage_detected" => "Damaged",
        "adoption" => "Adoption",
        "torrent_error" => "Torrent error",
        "recheck" => "Recheck",
        other => return other.replace('_', " "),
    }
    .to_owned()
}

/// Event time in the local zone: "HH:MM:SS" today, else "Mon DD HH:MM"
/// (web UI `formatEventTime`).
pub fn format_event_time(iso: &str) -> String {
    use chrono::{DateTime, Local};
    let Ok(t) = DateTime::parse_from_rfc3339(iso) else {
        return iso.to_owned();
    };
    let t = t.with_timezone(&Local);
    if t.date_naive() == Local::now().date_naive() {
        t.format("%H:%M:%S").to_string()
    } else {
        t.format("%b %-d %H:%M").to_string()
    }
}

pub fn render_event(e: EventRecord) -> gpui::AnyElement {
    div()
        .flex()
        .flex_row()
        .gap_2()
        .text_xs()
        .child(
            div()
                .w(px(100.))
                .flex_shrink_0()
                .text_color(theme::text_muted())
                .child(format_event_time(&e.time)),
        )
        .child(
            div()
                .w(px(100.))
                .flex_shrink_0()
                .text_color(severity_color(&e.severity))
                .child(kind_label(&e.kind)),
        )
        .child(div().flex_1().min_w(px(0.)).truncate().child(format!(
            "{}{}",
            e.message,
            match e.count {
                Some(c) if c > 1 => format!(" (×{c})"),
                _ => String::new(),
            }
        )))
        .into_any_element()
}

impl Render for DetailsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = [
            (Tab::Overview, "Overview"),
            (Tab::Files, "Files"),
            (Tab::Peers, "Peers"),
            (Tab::Events, "Events"),
        ];
        let body = match self.tab {
            Tab::Overview => self.render_overview(cx),
            Tab::Files => self.render_files(cx),
            Tab::Peers => self.render_peers(),
            Tab::Events => self.render_events(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .border_b_1()
                    .border_color(theme::border())
                    .children(tabs.iter().enumerate().map(|(i, (t, label))| {
                        let t = *t;
                        let active = self.tab == t;
                        div()
                            .id(("d-tab", i))
                            .px_3()
                            .py_1()
                            .text_sm()
                            .cursor_pointer()
                            .border_b_2()
                            .border_color(if active {
                                theme::primary()
                            } else {
                                gpui::transparent_black().into()
                            })
                            .text_color(if active {
                                theme::text()
                            } else {
                                theme::text_muted()
                            })
                            .hover(|s| s.text_color(theme::text()))
                            .on_click(cx.listener(move |p, _, _, cx| p.set_tab(t, cx)))
                            .child(*label)
                    }))
                    .child(div().flex_1())
                    .when_some(self.error.clone(), |d, e| {
                        d.child(
                            div()
                                .text_xs()
                                .truncate()
                                .max_w(px(600.))
                                .text_color(theme::error())
                                .child(e),
                        )
                    })
                    .when(self.error.is_none(), |d| {
                        d.children(
                            self.message
                                .clone()
                                .map(|m| div().text_xs().text_color(theme::success()).child(m)),
                        )
                    })
                    .child(
                        widgets::button("d-close", "×", true)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DetailsEvent::Close))),
                    ),
            )
            .child(
                div()
                    .id("d-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p_3()
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets() {
        // pieces 0..10: have 0,1,2,3 and 8
        let bits = [0b1111_0000, 0b1000_0000];
        assert_eq!(count_pieces(&bits, 10), 5);
        let b = piece_buckets(&bits, 10, 5);
        assert_eq!(b, vec![1.0, 1.0, 0.0, 0.0, 0.5]);
        assert_eq!(piece_buckets(&bits, 10, 100).len(), 10);
    }

    #[test]
    fn event_time() {
        assert_eq!(format_event_time("garbage"), "garbage");
        let s = format_event_time("2020-01-02T03:04:05Z");
        assert!(s.starts_with("Jan "), "{s}");
    }
}
