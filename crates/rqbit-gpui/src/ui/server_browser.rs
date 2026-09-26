//! "Browse server" picker (web UI `FilesystemBrowser` in select-torrents
//! mode): walk the server's allowed browse roots (`/fs/roots`,
//! `/fs/list`) and pick `.torrent` files, which the Add panel then adds
//! with `from_server_path`.

use std::collections::BTreeSet;

use gpui::{Context, EventEmitter, Window, div, prelude::*, px};

use super::{theme, widgets};
use crate::api::{ApiClient, FsListResponse, FsRoot};
use crate::format::format_bytes;

pub enum ServerBrowserEvent {
    /// Chosen .torrent paths on the server.
    Confirm(Vec<String>),
    Close,
}

pub struct ServerBrowser {
    client: ApiClient,
    roots: Option<Vec<FsRoot>>,
    listing: Option<FsListResponse>,
    selected: BTreeSet<String>,
    loading: bool,
    error: Option<String>,
    /// Bumped per navigation; stale listings are dropped.
    generation: u64,
}

impl EventEmitter<ServerBrowserEvent> for ServerBrowser {}

impl ServerBrowser {
    pub fn new(client: ApiClient, cx: &mut Context<Self>) -> Self {
        let c = client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(c.fs_roots()).await;
            this.update(cx, |b, cx| {
                match r {
                    Ok(r) => {
                        b.loading = false;
                        if let [only] = r.roots.as_slice() {
                            let p = only.path.clone();
                            b.open(p, cx);
                        }
                        b.roots = Some(r.roots);
                    }
                    Err(e) => {
                        b.loading = false;
                        b.error = Some(format!("Can't list server folders: {e:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            client,
            roots: None,
            listing: None,
            selected: BTreeSet::new(),
            loading: true,
            error: None,
            generation: 0,
        }
    }

    fn open(&mut self, path: String, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(client.fs_list(&path)).await;
            this.update(cx, |b, cx| {
                if b.generation != generation {
                    return;
                }
                b.loading = false;
                match r {
                    Ok(l) => {
                        b.error = None;
                        b.listing = Some(l);
                    }
                    Err(e) => b.error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn toggle(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
        cx.notify();
    }
}

impl Render for ServerBrowser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.listing.as_ref().map(|l| l.path.clone());
        let roots = self.roots.clone().unwrap_or_default();
        let torrents_here: Vec<String> = self
            .listing
            .iter()
            .flat_map(|l| l.entries.iter())
            .filter(|e| e.is_torrent)
            .map(|e| e.path.clone())
            .collect();
        let n = self.selected.len();

        let mut rows = Vec::new();
        if let Some(l) = &self.listing {
            if let Some(parent) = l.parent.clone() {
                rows.push(
                    div()
                        .id("fs-up")
                        .px_2()
                        .py_1()
                        .text_sm()
                        .cursor_pointer()
                        .text_color(theme::primary())
                        .hover(|s| s.bg(theme::surface_hover()))
                        .on_click(cx.listener(move |b, _, _, cx| b.open(parent.clone(), cx)))
                        .child("↑ ..")
                        .into_any_element(),
                );
            }
            for (i, e) in l
                .entries
                .iter()
                .filter(|e| e.is_dir || e.is_torrent)
                .enumerate()
            {
                let path = e.path.clone();
                if e.is_dir {
                    rows.push(
                        div()
                            .id(("fs-dir", i))
                            .px_2()
                            .py_1()
                            .text_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::surface_hover()))
                            .on_click(cx.listener(move |b, _, _, cx| b.open(path.clone(), cx)))
                            .child(format!("{}/", e.name))
                            .into_any_element(),
                    );
                } else {
                    let checked = self.selected.contains(&e.path);
                    rows.push(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .child(
                                widgets::checkbox(("fs-file", i), e.name.clone(), checked, true)
                                    .on_click(
                                        cx.listener(move |b, _, _, cx| b.toggle(path.clone(), cx)),
                                    ),
                            )
                            .child(div().flex_1())
                            .children(e.size.map(|s| {
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(format_bytes(s))
                            }))
                            .into_any_element(),
                    );
                }
            }
            if l.entries.iter().all(|e| !e.is_dir && !e.is_torrent) {
                rows.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child("No folders or .torrent files here.")
                        .into_any_element(),
                );
            }
            if l.truncated == Some(true) {
                rows.push(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(theme::warning())
                        .child("Listing truncated (too many entries).")
                        .into_any_element(),
                );
            }
        }

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
                    .items_center()
                    .gap_1()
                    .child(div().text_sm().pr_1().child("Server folders"))
                    .children(roots.iter().enumerate().map(|(i, r)| {
                        let path = r.path.clone();
                        let active = current.as_deref().is_some_and(|c| c.starts_with(&r.path));
                        widgets::segment(("fs-root", i), r.label.clone(), active)
                            .rounded_md()
                            .on_click(cx.listener(move |b, _, _, cx| b.open(path.clone(), cx)))
                    }))
                    .child(div().flex_1())
                    .child(
                        widgets::button("fs-close", "×", true).on_click(
                            cx.listener(|_, _, _, cx| cx.emit(ServerBrowserEvent::Close)),
                        ),
                    ),
            )
            .children(current.map(|p| {
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .truncate()
                    .child(p)
            }))
            .children(
                self.error
                    .clone()
                    .map(|e| div().text_xs().text_color(theme::error()).child(e)),
            )
            .child(
                div()
                    .id("fs-list")
                    .flex()
                    .flex_col()
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg())
                    .when(rows.is_empty(), |d| {
                        d.child(div().p_2().text_xs().text_color(theme::text_muted()).child(
                            if self.loading {
                                "Loading…"
                            } else {
                                "Choose a folder."
                            },
                        ))
                    })
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .when(!torrents_here.is_empty(), |d| {
                        d.child(
                            widgets::link("fs-all", "select all .torrent here").on_click(
                                cx.listener(move |b, _, _, cx| {
                                    b.selected.extend(torrents_here.iter().cloned());
                                    cx.notify();
                                }),
                            ),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(format!("{n} selected")),
                    )
                    .child(
                        widgets::primary_button("fs-confirm", "Add to queue", n > 0).when(
                            n > 0,
                            |b| {
                                b.on_click(cx.listener(|b, _, _, cx| {
                                    let paths = std::mem::take(&mut b.selected);
                                    cx.emit(ServerBrowserEvent::Confirm(
                                        paths.into_iter().collect(),
                                    ));
                                    cx.notify();
                                }))
                            },
                        ),
                    ),
            )
    }
}
