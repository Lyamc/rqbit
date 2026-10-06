//! Orphaned-download cleanup (web UI `CleanupModal`): pick scan roots, run a
//! dry-run scan (a read-only GET), tick items (nothing is ticked by
//! default), then quarantine them (default, restorable) or delete them
//! permanently after a confirm. Quarantine batches can be restored or
//! purged; a scheduled scan (hours) only reports.

use std::collections::HashSet;

use gpui::{Context, Entity, EventEmitter, Window, div, prelude::*, px};

use super::text_input::TextInput;
use super::{theme, widgets};
use crate::api::{
    ApiClient, CleanupApplyOutcome, CleanupItem, CleanupRoot, CleanupRootsResponse, CleanupScan,
    QuarantineBatch,
};
use crate::format::format_bytes;
use crate::time::{SystemTime, UNIX_EPOCH};

pub enum CleanupPanelEvent {
    Close,
}

/// Path shown relative to its scan root ("Show/ep1.mkv").
pub fn rel_path<'a>(path: &'a str, root: &str) -> &'a str {
    if path == root {
        return path;
    }
    for sep in ['/', '\\'] {
        let r = root.trim_end_matches(sep);
        if let Some(rest) = path.strip_prefix(r).and_then(|p| p.strip_prefix(sep)) {
            return rest;
        }
    }
    path
}

/// "5 min ago", "3 h ago", "12 d ago", "2 y ago" (floored, like the web UI).
pub fn format_age(mtime: Option<u64>, now: u64) -> String {
    let Some(m) = mtime else {
        return "—".to_owned();
    };
    let d = now.saturating_sub(m);
    if d < 3600 {
        format!("{} min ago", d / 60)
    } else if d < 86_400 {
        format!("{} h ago", d / 3600)
    } else if d < 365 * 86_400 {
        format!("{} d ago", d / 86_400)
    } else {
        format!("{} y ago", d / (365 * 86_400))
    }
}

/// Scheduled-scan hours field: empty / 0 = off (`Ok(None)`).
pub fn parse_scan_hours(v: &str) -> Result<Option<u64>, ()> {
    let t = v.trim();
    if t.is_empty() {
        return Ok(None);
    }
    match t.parse::<u64>() {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(n)),
        Err(_) => Err(()),
    }
}

/// (count, bytes, files) of the ticked items.
pub fn selection_summary(items: &[CleanupItem], selected: &HashSet<u32>) -> (usize, u64, u64) {
    items
        .iter()
        .filter(|i| selected.contains(&i.id))
        .fold((0, 0, 0), |(c, b, f), i| (c + 1, b + i.size, f + i.files))
}

fn local_time(secs: u64) -> String {
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub struct CleanupPanel {
    client: ApiClient,
    info: Option<CleanupRootsResponse>,
    roots: Vec<CleanupRoot>,
    ticked: HashSet<String>,
    add_root: Entity<TextInput>,
    min_age: Entity<TextInput>,
    hours: Entity<TextInput>,
    hours_msg: Option<String>,
    scanning: bool,
    scan: Option<CleanupScan>,
    selected: HashSet<u32>,
    delete: bool,
    confirming: bool,
    understood: bool,
    applying: bool,
    outcome: Option<CleanupApplyOutcome>,
    batches: Vec<QuarantineBatch>,
    purge_ask: Option<String>,
    batch_busy: Option<String>,
    error: Option<String>,
}

impl EventEmitter<CleanupPanelEvent> for CleanupPanel {}

impl CleanupPanel {
    pub fn new(client: ApiClient, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            client,
            info: None,
            roots: Vec::new(),
            ticked: HashSet::new(),
            add_root: cx.new(|cx| TextInput::new("", "/path/to/folder", cx)),
            min_age: cx.new(|cx| TextInput::new("60", "60", cx)),
            hours: cx.new(|cx| TextInput::new("", "off", cx)),
            hours_msg: None,
            scanning: false,
            scan: None,
            selected: HashSet::new(),
            delete: false,
            confirming: false,
            understood: false,
            applying: false,
            outcome: None,
            batches: Vec::new(),
            purge_ask: None,
            batch_busy: None,
            error: None,
        };
        this.load_roots(cx);
        this.load_quarantine(cx);
        this
    }

    fn load_roots(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(client.cleanup_roots()).await;
            this.update(cx, |p, cx| {
                match r {
                    Ok(info) => {
                        p.ticked = info
                            .roots
                            .iter()
                            .filter(|r| r.default_on && r.exists)
                            .map(|r| r.path.clone())
                            .collect();
                        p.roots = info.roots.clone();
                        let age = info.min_age_minutes.to_string();
                        p.min_age.update(cx, |t, cx| t.set_text(age, cx));
                        let hours = info.scan_hours.map(|h| h.to_string()).unwrap_or_default();
                        p.hours.update(cx, |t, cx| t.set_text(hours, cx));
                        p.info = Some(info);
                    }
                    Err(e) => p.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_quarantine(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.cleanup_quarantine())
                .await;
            this.update(cx, |p, cx| {
                if let Ok(q) = r {
                    p.batches = q.batches;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_scan(&mut self, s: CleanupScan) {
        self.scan = Some(s);
        self.selected.clear();
        self.confirming = false;
        self.outcome = None;
    }

    fn run_scan(&mut self, cx: &mut Context<Self>) {
        let age = self.min_age.read(cx).text().trim().to_owned();
        let min_age = if age.is_empty() {
            None
        } else {
            match age.parse::<u64>() {
                Ok(v) => Some(v),
                Err(_) => {
                    self.error = Some("Minimum age must be whole minutes.".into());
                    cx.notify();
                    return;
                }
            }
        };
        let roots: Vec<String> = self
            .roots
            .iter()
            .filter(|r| self.ticked.contains(&r.path))
            .map(|r| r.path.clone())
            .collect();
        if roots.is_empty() {
            return;
        }
        self.scanning = true;
        self.error = None;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.cleanup_scan(&roots, min_age))
                .await;
            this.update(cx, |p, cx| {
                p.scanning = false;
                match r {
                    Ok(s) => p.set_scan(s),
                    Err(e) => p.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn show_latest(&mut self, cx: &mut Context<Self>) {
        let id = self
            .info
            .as_ref()
            .and_then(|i| i.latest_scan.as_ref())
            .map(|s| s.scan_id.clone());
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.cleanup_scan_result(id.as_deref()))
                .await;
            this.update(cx, |p, cx| {
                match r {
                    Ok(s) => p.set_scan(s),
                    Err(e) => p.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let Some(scan) = &self.scan else {
            return;
        };
        let scan_id = scan.scan_id.clone();
        let ids: Vec<u32> = scan
            .items
            .iter()
            .filter(|i| self.selected.contains(&i.id))
            .map(|i| i.id)
            .collect();
        let delete = self.delete;
        self.applying = true;
        self.error = None;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(client.cleanup_apply(&scan_id, &ids, delete))
                .await;
            this.update(cx, |p, cx| {
                p.applying = false;
                match r {
                    Ok(o) => {
                        let done: HashSet<u32> =
                            o.results.iter().filter(|r| r.ok).filter_map(|r| r.id).collect();
                        if let Some(s) = &mut p.scan {
                            s.items.retain(|i| !done.contains(&i.id));
                            s.total_bytes = s.items.iter().map(|i| i.size).sum();
                        }
                        p.selected.clear();
                        p.confirming = false;
                        p.understood = false;
                        p.outcome = Some(o);
                        p.load_quarantine(cx);
                    }
                    Err(e) => p.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn batch_action(&mut self, id: String, purge: bool, cx: &mut Context<Self>) {
        self.batch_busy = Some(id.clone());
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let fut = if purge {
                client.cleanup_purge(&id)
            } else {
                client.cleanup_restore(&id)
            };
            let r = cx.background_executor().spawn(fut).await;
            this.update(cx, |p, cx| {
                p.batch_busy = None;
                p.purge_ask = None;
                if let Err(e) = r {
                    p.error = Some(format!("{e:#}"));
                }
                p.load_quarantine(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn save_hours(&mut self, cx: &mut Context<Self>) {
        let Ok(h) = parse_scan_hours(self.hours.read(cx).text()) else {
            self.hours_msg = Some("Enter whole hours, or leave empty for off.".into());
            cx.notify();
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let mut prefs = client.get_preferences().await?;
                    prefs.insert("cleanup_scan_hours".into(), serde_json::json!(h));
                    client.set_preferences(&prefs).await
                })
                .await;
            this.update(cx, |p, cx| {
                p.hours_msg = Some(match (r, h) {
                    (Err(e), _) => format!("{e:#}"),
                    (Ok(()), Some(h)) => format!("Saved: scans every {h} h (report only)."),
                    (Ok(()), None) => "Saved: off.".into(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn add_root(&mut self, cx: &mut Context<Self>) {
        let p = self.add_root.read(cx).text().trim().to_owned();
        if p.is_empty() {
            return;
        }
        if !self.roots.iter().any(|r| r.path == p) {
            self.roots.push(CleanupRoot {
                path: p.clone(),
                kind: "custom".into(),
                default_on: true,
                exists: true,
            });
        }
        self.ticked.insert(p);
        self.add_root.update(cx, |t, cx| t.set_text("", cx));
        cx.notify();
    }

    fn render_roots(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let latest = self
            .info
            .as_ref()
            .and_then(|i| i.latest_scan.clone())
            .filter(|_| self.scan.is_none());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Folders to scan"))
            .children(self.roots.iter().enumerate().map(|(i, r)| {
                let path = r.path.clone();
                let kind = match r.kind.as_str() {
                    "download" => "download folder",
                    "completion" => "completion folder",
                    "organize" => "organize root",
                    _ => "custom",
                };
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        widgets::checkbox(("cl-root", i), r.path.clone(), self.ticked.contains(&r.path), r.exists)
                            .when(r.exists, |d| {
                                d.on_click(cx.listener(move |p, _, _, cx| {
                                    if !p.ticked.remove(&path) {
                                        p.ticked.insert(path.clone());
                                    }
                                    cx.notify();
                                }))
                            }),
                    )
                    .child(div().text_xs().text_color(theme::text_muted()).child(if r.exists {
                        kind.to_owned()
                    } else {
                        format!("{kind} (missing)")
                    }))
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child(div().w(px(260.)).child(self.add_root.clone()))
                    .child(
                        widgets::button("cl-add-root", "Add folder", true)
                            .on_click(cx.listener(|p, _, _, cx| p.add_root(cx))),
                    )
                    .child(div().w(px(12.)))
                    .child("Ignore anything changed in the last")
                    .child(div().w(px(120.)).child(self.min_age.clone()))
                    .child("minutes")
                    .child({
                        let enabled = !self.scanning && !self.ticked.is_empty();
                        widgets::primary_button(
                            "cl-scan",
                            if self.scanning { "Scanning…" } else { "Scan (dry run)" },
                            enabled,
                        )
                        .when(enabled, |b| b.on_click(cx.listener(|p, _, _, cx| p.run_scan(cx))))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child("Extra folders must be inside the download folder, a completion folder, or a server browse root."),
            )
            .when_some(latest, |d, l| {
                d.child(
                    widgets::link(
                        "cl-latest",
                        format!(
                            "{} {}: {} item(s), {} — show",
                            if l.scheduled { "Scheduled scan" } else { "Last scan" },
                            local_time(l.time),
                            l.items,
                            format_bytes(l.total_bytes)
                        ),
                    )
                    .on_click(cx.listener(|p, _, _, cx| p.show_latest(cx))),
                )
            })
    }

    fn render_results(&self, scan: &CleanupScan, cx: &mut Context<Self>) -> impl IntoElement {
        let now = now_secs();
        let multi_root = scan.roots.len() > 1;
        let all = !scan.items.is_empty() && self.selected.len() == scan.items.len();
        let (count, bytes, files) = selection_summary(&scan.items, &self.selected);
        let sk = &scan.skipped;
        let title = if scan.items.is_empty() {
            "Nothing orphaned found".to_owned()
        } else {
            format!(
                "{} orphaned item(s), {}",
                scan.items.len(),
                format_bytes(scan.total_bytes)
            )
        };
        let sub = format!(
            "{} {} · checked against {} torrent(s) · skipped {} recent, {} hidden/system, {} symlink(s){}",
            if scan.scheduled { "scheduled scan" } else { "dry run" },
            local_time(scan.time),
            scan.torrents_checked,
            sk.recent,
            sk.hidden,
            sk.symlinks,
            if sk.truncated { " · list truncated" } else { "" }
        );
        let ids: Vec<u32> = scan.items.iter().map(|i| i.id).collect();
        let rows: Vec<_> = scan
            .items
            .iter()
            .map(|it| {
                let id = it.id;
                let sel = self.selected.contains(&id);
                let rel = rel_path(&it.path, &it.root).to_owned();
                let label = format!(
                    "{}{}{}",
                    if it.kind == "dir" { "📁 " } else { "" },
                    rel,
                    if multi_root { format!(" ({})", it.root) } else { String::new() }
                );
                div()
                    .id(("cl-item", id as usize))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .border_t_1()
                    .border_color(theme::border())
                    .cursor_pointer()
                    .when(sel, |d| d.bg(theme::surface_hover()))
                    .on_click(cx.listener(move |p, _, _, cx| {
                        if !p.selected.remove(&id) {
                            p.selected.insert(id);
                        }
                        cx.notify();
                    }))
                    .child(widgets::checkbox(("cl-item-cb", id as usize), "", sel, true))
                    .child(div().flex_1().min_w(px(0.)).truncate().font_family("monospace").child(label))
                    .child(
                        div()
                            .w(px(130.))
                            .flex_shrink_0()
                            .text_right()
                            .child(if it.kind == "dir" {
                                format!("{} · {} files", format_bytes(it.size), it.files)
                            } else {
                                format_bytes(it.size)
                            }),
                    )
                    .child(div().w(px(80.)).flex_shrink_0().child(format_age(it.mtime, now)))
                    .child(
                        div()
                            .w(px(300.))
                            .flex_shrink_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(it.reason.clone()),
                    )
                    .into_any_element()
            })
            .collect();
        let delete = self.delete;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_baseline()
                    .gap_2()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                    .child(div().text_xs().text_color(theme::text_muted()).child(sub)),
            )
            .when(!sk.errors.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme::warning())
                        .child(format!("Couldn't read: {}", sk.errors.iter().take(3).cloned().collect::<Vec<_>>().join("; "))),
                )
            })
            .when(!scan.items.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(
                                    widgets::checkbox("cl-all", "", all, true).on_click(cx.listener(
                                        move |p, _, _, cx| {
                                            if all {
                                                p.selected.clear();
                                            } else {
                                                p.selected = ids.iter().copied().collect();
                                            }
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(div().flex_1().child("Path"))
                                .child(div().w(px(130.)).text_right().child("Size"))
                                .child(div().w(px(80.)).child("Modified"))
                                .child(div().w(px(300.)).child("Why")),
                        )
                        .children(rows),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .child(
                                    widgets::segment("cl-act-q", "Move to quarantine (can be restored)", !delete)
                                        .on_click(cx.listener(|p, _, _, cx| {
                                            p.delete = false;
                                            p.confirming = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    widgets::segment("cl-act-d", "Delete permanently", delete).on_click(
                                        cx.listener(|p, _, _, cx| {
                                            p.delete = true;
                                            p.confirming = false;
                                            cx.notify();
                                        }),
                                    ),
                                ),
                        )
                        .child({
                            let enabled = count > 0 && !self.applying;
                            let label = format!(
                                "{} {count} item(s) ({})…",
                                if delete { "Delete" } else { "Quarantine" },
                                format_bytes(bytes)
                            );
                            let b = if delete && enabled {
                                widgets::danger_button("cl-apply", label)
                            } else {
                                widgets::primary_button("cl-apply", label, enabled)
                            };
                            b.when(enabled, |b| {
                                b.on_click(cx.listener(|p, _, _, cx| {
                                    p.confirming = true;
                                    cx.notify();
                                }))
                            })
                        }),
                )
            })
            .when(self.confirming, |d| {
                let can = !self.applying && (!delete || self.understood);
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(if delete { theme::error() } else { theme::border() })
                        .text_sm()
                        .when(delete, |d| {
                            d.child(
                                div()
                                    .text_color(theme::error())
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(format!(
                                        "Permanently delete {count} item(s), {files} file(s), {}? This can't be undone.",
                                        format_bytes(bytes)
                                    )),
                            )
                            .child(
                                widgets::checkbox("cl-understood", "I understand these files will be gone", self.understood, true)
                                    .on_click(cx.listener(|p, _, _, cx| {
                                        p.understood = !p.understood;
                                        cx.notify();
                                    })),
                            )
                        })
                        .when(!delete, |d| {
                            d.child(format!(
                                "Move {count} item(s), {} into .rqbit-quarantine inside each scanned folder? You can restore them below, or delete them later.",
                                format_bytes(bytes)
                            ))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(widgets::button("cl-confirm-no", "Cancel", true).on_click(
                                    cx.listener(|p, _, _, cx| {
                                        p.confirming = false;
                                        cx.notify();
                                    }),
                                ))
                                .child({
                                    let label = if self.applying {
                                        "Working…"
                                    } else if delete {
                                        "Delete permanently"
                                    } else {
                                        "Move to quarantine"
                                    };
                                    let b = if delete && can {
                                        widgets::danger_button("cl-confirm-yes", label)
                                    } else {
                                        widgets::primary_button("cl-confirm-yes", label, can)
                                    };
                                    b.when(can, |b| b.on_click(cx.listener(|p, _, _, cx| p.apply(cx))))
                                }),
                        ),
                )
            })
    }

    fn render_quarantine(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Quarantine"))
            .when(self.batches.is_empty(), |d| {
                d.child(div().text_sm().text_color(theme::text_muted()).child("Empty."))
            })
            .children(self.batches.iter().enumerate().map(|(i, b)| {
                let bytes: u64 = b.items.iter().map(|i| i.size).sum();
                let busy = self.batch_busy.as_deref() == Some(b.id.as_str());
                let asking = self.purge_ask.as_deref() == Some(b.id.as_str());
                let (id1, id2, id3) = (b.id.clone(), b.id.clone(), b.id.clone());
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .pb_1()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_sm()
                    .child(local_time(b.created))
                    .child(
                        div()
                            .text_color(theme::text_muted())
                            .child(format!("{} item(s), {} in {}", b.items.len(), format_bytes(bytes), b.dir)),
                    )
                    .child(
                        widgets::button(("cl-restore", i), "Restore", !busy).when(!busy, |x| {
                            x.on_click(cx.listener(move |p, _, _, cx| p.batch_action(id1.clone(), false, cx)))
                        }),
                    )
                    .when(!asking, |d| {
                        d.child(widgets::button(("cl-purge", i), "Delete…", true).on_click(cx.listener(
                            move |p, _, _, cx| {
                                p.purge_ask = Some(id2.clone());
                                cx.notify();
                            },
                        )))
                    })
                    .when(asking, |d| {
                        d.child(div().text_color(theme::error()).child("Delete these permanently?"))
                            .child(
                                widgets::danger_button(("cl-purge-yes", i), "Delete permanently").when(!busy, |x| {
                                    x.on_click(cx.listener(move |p, _, _, cx| p.batch_action(id3.clone(), true, cx)))
                                }),
                            )
                            .child(widgets::button(("cl-purge-no", i), "Cancel", true).on_click(cx.listener(
                                |p, _, _, cx| {
                                    p.purge_ask = None;
                                    cx.notify();
                                },
                            )))
                    })
            }))
    }
}

impl Render for CleanupPanel {
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
                    .child("Clean up orphaned downloads"),
            )
            .child(div().flex_1())
            .child(
                widgets::button("cl-close-x", "×", true)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CleanupPanelEvent::Close))),
            );
        let scan = self.scan.clone();
        let outcome = self.outcome.clone().map(|o| {
            div()
                .flex()
                .flex_col()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(if o.failed > 0 { theme::warning() } else { theme::success() })
                .text_sm()
                .child(format!(
                    "{} {} item(s), {}.",
                    if o.action == "delete" { "Deleted" } else { "Moved to quarantine" },
                    o.ok,
                    format_bytes(o.bytes)
                ))
                .children(
                    o.results
                        .iter()
                        .filter(|r| !r.ok)
                        .map(|r| {
                            div().text_xs().child(format!(
                                "{}: {}",
                                r.path,
                                r.error.clone().unwrap_or_default()
                            ))
                        })
                        .collect::<Vec<_>>(),
                )
        });
        let hours_row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .text_sm()
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Scheduled scan"))
            .child("every")
            .child(div().w(px(120.)).child(self.hours.clone()))
            .child("hours — reports only (Events + here), never moves or deletes.")
            .child(
                widgets::button("cl-hours-save", "Save", true)
                    .on_click(cx.listener(|p, _, _, cx| p.save_hours(cx))),
            )
            .children(
                self.hours_msg
                    .clone()
                    .map(|m| div().text_xs().text_color(theme::text_muted()).child(m)),
            );
        div().flex().flex_col().size_full().child(header).child(
            div()
                .id("cl-body")
                .flex()
                .flex_col()
                .gap_4()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .p_4()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("Finds files and folders in your download locations that no torrent in rqbit uses. Scanning changes nothing. Hidden/system files, symlinks and anything changed recently are never touched; every item is checked again before it is moved or deleted."),
                )
                .child(self.render_roots(cx))
                .children(
                    self.error
                        .clone()
                        .map(|e| div().text_sm().text_color(theme::error()).child(e)),
                )
                .children(outcome)
                .when_some(scan, |d, s| d.child(self.render_results(&s, cx)))
                .child(self.render_quarantine(cx))
                .child(hours_row),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_strips_root() {
        assert_eq!(rel_path("/d/Show/ep1.mkv", "/d"), "Show/ep1.mkv");
        assert_eq!(rel_path("/d/Show", "/d/"), "Show");
        assert_eq!(rel_path("D:\\dl\\x.iso", "D:\\dl"), "x.iso");
        assert_eq!(rel_path("/other/x", "/d"), "/other/x");
        assert_eq!(rel_path("/dd/x", "/d"), "/dd/x");
        assert_eq!(rel_path("/d", "/d"), "/d");
    }

    #[test]
    fn age_is_floored() {
        assert_eq!(format_age(None, 100), "—");
        assert_eq!(format_age(Some(1000), 1000 + 119), "1 min ago");
        assert_eq!(format_age(Some(0), 3599), "59 min ago");
        assert_eq!(format_age(Some(0), 3600 * 23 + 3599), "23 h ago");
        assert_eq!(format_age(Some(0), 86_400 * 2 - 1), "1 d ago");
        assert_eq!(format_age(Some(0), 365 * 86_400 * 2), "2 y ago");
        assert_eq!(format_age(Some(500), 100), "0 min ago");
    }

    #[test]
    fn scan_hours_parsing() {
        assert_eq!(parse_scan_hours(""), Ok(None));
        assert_eq!(parse_scan_hours(" 0 "), Ok(None));
        assert_eq!(parse_scan_hours("24"), Ok(Some(24)));
        assert!(parse_scan_hours("1.5").is_err());
        assert!(parse_scan_hours("-1").is_err());
    }

    #[test]
    fn selection_totals() {
        let item = |id, size, files| CleanupItem {
            id,
            size,
            files,
            ..Default::default()
        };
        let items = vec![item(1, 10, 1), item(2, 20, 3), item(3, 5, 1)];
        let sel: HashSet<u32> = [1, 2, 9].into_iter().collect();
        assert_eq!(selection_summary(&items, &sel), (2, 30, 4));
        assert_eq!(selection_summary(&items, &HashSet::new()), (0, 0, 0));
    }
}
