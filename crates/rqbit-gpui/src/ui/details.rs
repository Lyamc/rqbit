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
use super::torrent_table::{damage_short_text, damage_text_color};
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
    /// Open the Events view filtered to this torrent (info hash, name).
    OpenEvents(String, String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Overview,
    Files,
    Peers,
    Events,
    Rules,
}

/// Right-click menu on file rows.
struct FileMenu {
    pos: gpui::Point<gpui::Pixels>,
    ids: Vec<usize>,
    open_sub: Option<usize>,
}

/// One editable rule field in the Rules tab.
struct RuleInput {
    /// Path inside the override ("stalled.after_secs").
    path: &'static str,
    kind: RuleKind,
    input: Entity<TextInput>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    DurationReq,
    Duration,
    Size,
    Float,
    Int,
}

const RULE_FIELDS: [(&str, RuleKind, &str, &str); 7] = [
    ("stalled.after_secs", RuleKind::DurationReq, "No verified progress for", "24h"),
    ("seeding.max_seed_secs", RuleKind::Duration, "Seeding time", "e.g. 7d"),
    ("seeding.max_uploaded_bytes", RuleKind::Size, "Uploaded", "e.g. 50 GB"),
    ("seeding.max_ratio", RuleKind::Float, "Ratio", "e.g. 2.0"),
    ("speed_window.full_speed_secs", RuleKind::Duration, "Full speed for", "e.g. 2h"),
    ("speed_window.full_speed_bytes", RuleKind::Size, "or until uploaded", "e.g. 10 GB"),
    ("speed_window.cap_kib_per_sec", RuleKind::Int, "then cap at (KB/s)", "100"),
];

const RULE_SECTIONS: [(&str, &str); 3] = [
    ("stalled", "Stalled / no progress"),
    ("seeding", "Seeding limits"),
    ("speed_window", "Full-speed window"),
];

fn rule_actions(section: &str) -> &'static [(&'static str, &'static str)] {
    match section {
        "stalled" => &[
            ("flag", "Flag"),
            ("pause", "Pause"),
            ("remove_keep", "Remove (keep)"),
            ("remove_finish", "Remove (finish)"),
            ("remove_delete", "Remove + delete"),
            ("remove_policy", "Per policy"),
        ],
        "seeding" => &[
            ("pause", "Pause"),
            ("remove_keep", "Remove (keep)"),
            ("remove_policy", "Per policy"),
        ],
        _ => &[("cap", "Cap upload"), ("stop", "Stop seeding")],
    }
}

/// One-line summary of a (global) rule section.
fn summarize_rule(section: &str, r: &serde_json::Map<String, Value>) -> String {
    if !r.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        return "Off".into();
    }
    let d = |k: &str| r.get(k).and_then(Value::as_u64).map(crate::format::format_duration);
    let b = |k: &str| r.get(k).and_then(Value::as_u64).map(format_bytes);
    let act = r.get("action").and_then(Value::as_str).unwrap_or("pause");
    match section {
        "stalled" => format!("After {} without progress → {act}", d("after_secs").unwrap_or_default()),
        "seeding" => {
            let mut lim = Vec::new();
            if let Some(x) = d("max_seed_secs") {
                lim.push(x);
            }
            if let Some(x) = b("max_uploaded_bytes") {
                lim.push(x);
            }
            if let Some(x) = r.get("max_ratio").and_then(Value::as_f64) {
                lim.push(format!("ratio {x}"));
            }
            format!("{} → {act}", if lim.is_empty() { "no limit set".into() } else { lim.join(" / ") })
        }
        _ => {
            let then = if r.get("then").and_then(Value::as_str) == Some("stop") {
                "stop".to_owned()
            } else {
                format!("cap {} KB/s", r.get("cap_kib_per_sec").and_then(Value::as_u64).unwrap_or(100))
            };
            let mut lim = Vec::new();
            if let Some(x) = d("full_speed_secs") {
                lim.push(x);
            }
            if let Some(x) = b("full_speed_bytes") {
                lim.push(x);
            }
            format!("Full speed for {} → {then}", if lim.is_empty() { "—".into() } else { lim.join(" / ") })
        }
    }
}

fn rule_text(kind: RuleKind, v: Option<&Value>) -> String {
    match (kind, v) {
        (_, None | Some(Value::Null)) => String::new(),
        (RuleKind::Duration | RuleKind::DurationReq, Some(v)) => {
            v.as_u64().map(crate::format::format_duration).unwrap_or_default()
        }
        (RuleKind::Size, Some(v)) => v.as_u64().map(format_bytes).unwrap_or_default(),
        (_, Some(v)) => v.to_string(),
    }
}

/// Parses a rule input into JSON (Err = message). Empty → null / unchanged.
fn rule_value(kind: RuleKind, label: &str, text: &str) -> Result<Option<Value>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(match kind {
            RuleKind::DurationReq | RuleKind::Int => None,
            _ => Some(Value::Null),
        });
    }
    let bad = || format!("{label}: \"{t}\" is not valid");
    Ok(Some(match kind {
        RuleKind::Duration | RuleKind::DurationReq => {
            serde_json::json!(crate::format::parse_duration(t, 60).ok_or_else(bad)?)
        }
        RuleKind::Size => serde_json::json!(crate::format::parse_size(t).ok_or_else(bad)?),
        RuleKind::Float => {
            let v: f64 = t.parse().map_err(|_| bad())?;
            if v > 0.0 { serde_json::json!(v) } else { Value::Null }
        }
        RuleKind::Int => serde_json::json!(t.parse::<u64>().map_err(|_| bad())?),
    }))
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
    /// Highlighted files (click / ctrl / shift), separate from "included".
    file_sel: HashSet<usize>,
    file_anchor: Option<usize>,
    file_menu: Option<FileMenu>,
    order: Option<crate::api::DownloadOrderView>,
    rules: Option<crate::api::TorrentRulesView>,
    /// Override being edited (object with stalled / seeding / speed_window).
    rules_draft: serde_json::Map<String, Value>,
    rules_dirty: bool,
    rule_inputs: Vec<RuleInput>,
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
            file_sel: HashSet::new(),
            file_anchor: None,
            file_menu: None,
            order: None,
            rules: None,
            rules_draft: serde_json::Map::new(),
            rules_dirty: false,
            rule_inputs: Vec::new(),
            _poll: poll,
        };
        this.load_prefs(cx);
        this.load_events(cx);
        this
    }

    pub fn id(&self) -> usize {
        self.id
    }

    /// Show the "Move" form (right-click → Move files…).
    pub fn open_move(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Overview;
        self.move_open = true;
        if let Some(t) = &self.torrent {
            let f = save_folder_of(&t.output_folder, t.name.as_deref());
            self.move_input.update(cx, |i, cx| i.set_text(f, cx));
        }
        cx.notify();
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
            Tab::Rules if n.is_multiple_of(3) => self.load_rules(false, cx),
            _ => {}
        }
    }

    fn load_order(&mut self, cx: &mut Context<Self>) {
        let (client, id) = (self.client.clone(), self.id);
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(client.get_download_order(id)).await;
            this.update(cx, |p, cx| {
                if let Ok(v) = r {
                    p.order = Some(v);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_order(&mut self, patch: Value, cx: &mut Context<Self>) {
        let fut = self.client.set_download_order(self.id, &patch);
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(fut).await;
            this.update(cx, |p, cx| {
                match r {
                    Ok(v) => {
                        p.message = Some(format!("Download order: {}", v.summary));
                        p.order = Some(v);
                    }
                    Err(e) => p.error = Some(format!("Download order failed: {e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_rules(&mut self, reset_draft: bool, cx: &mut Context<Self>) {
        let (client, id) = (self.client.clone(), self.id);
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(client.get_rules(id)).await;
            this.update(cx, |p, cx| {
                match r {
                    Ok(v) => {
                        if reset_draft || (!p.rules_dirty && p.rules.is_none()) {
                            p.rules_draft = v
                                .override_
                                .as_ref()
                                .and_then(|o| o.as_object().cloned())
                                .unwrap_or_default();
                            p.rules_dirty = false;
                            p.rules = Some(v);
                            p.rebuild_rule_inputs(cx);
                        } else {
                            p.rules = Some(v);
                        }
                    }
                    Err(e) => p.error = Some(format!("Rules: {e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Text inputs for the overridden sections (values from the draft).
    fn rebuild_rule_inputs(&mut self, cx: &mut Context<Self>) {
        let mut inputs = Vec::new();
        for (path, kind, _, placeholder) in RULE_FIELDS.iter() {
            let (section, key) = path.split_once('.').expect("dotted");
            let Some(sec) = self.rules_draft.get(section).and_then(Value::as_object) else {
                continue;
            };
            let text = rule_text(*kind, sec.get(key));
            inputs.push(RuleInput {
                path,
                kind: *kind,
                input: cx.new(|cx| TextInput::new(text, *placeholder, cx)),
            });
        }
        self.rule_inputs = inputs;
    }

    fn toggle_rule_override(&mut self, section: &'static str, cx: &mut Context<Self>) {
        self.sync_rule_inputs(cx).ok();
        if self.rules_draft.remove(section).is_none() {
            let global = self
                .rules
                .as_ref()
                .and_then(|r| r.global.get(section).cloned())
                .unwrap_or_else(|| Value::Object(Default::default()));
            self.rules_draft.insert(section.to_owned(), global);
        }
        self.rules_dirty = true;
        self.rebuild_rule_inputs(cx);
        cx.notify();
    }

    fn set_rule_field(&mut self, section: &str, key: &str, v: Value, cx: &mut Context<Self>) {
        self.sync_rule_inputs(cx).ok();
        if let Some(Value::Object(sec)) = self.rules_draft.get_mut(section) {
            sec.insert(key.to_owned(), v);
            self.rules_dirty = true;
        }
        cx.notify();
    }

    /// Copies the text inputs into the draft.
    fn sync_rule_inputs(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let mut errors = Vec::new();
        for ri in &self.rule_inputs {
            let (section, key) = ri.path.split_once('.').expect("dotted");
            let label = RULE_FIELDS.iter().find(|f| f.0 == ri.path).map(|f| f.2).unwrap_or(ri.path);
            match rule_value(ri.kind, label, ri.input.read(cx).text()) {
                Ok(Some(v)) => {
                    if let Some(Value::Object(sec)) = self.rules_draft.get_mut(section)
                        && sec.get(key) != Some(&v)
                    {
                        sec.insert(key.to_owned(), v);
                        self.rules_dirty = true;
                    }
                }
                Ok(None) => {}
                Err(e) => errors.push(e),
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }

    fn save_rules(&mut self, reset: bool, cx: &mut Context<Self>) {
        let body = if reset {
            Value::Null
        } else {
            if let Err(e) = self.sync_rule_inputs(cx) {
                self.error = Some(e);
                cx.notify();
                return;
            }
            if self.rules_draft.is_empty() {
                Value::Null
            } else {
                Value::Object(self.rules_draft.clone())
            }
        };
        let fut = self.client.set_rules(self.id, body);
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(fut).await;
            this.update(cx, |p, cx| {
                match r {
                    Ok(v) => {
                        p.message = Some("Rules saved.".into());
                        p.error = None;
                        p.rules_draft = v
                            .override_
                            .as_ref()
                            .and_then(|o| o.as_object().cloned())
                            .unwrap_or_default();
                        p.rules_dirty = false;
                        p.rules = Some(v);
                        p.rebuild_rule_inputs(cx);
                    }
                    Err(e) => p.error = Some(format!("Saving rules failed: {e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn file_clicked(&mut self, i: usize, m: gpui::Modifiers, cx: &mut Context<Self>) {
        let cmd = m.control || m.platform;
        if m.shift
            && let Some(a) = self.file_anchor
        {
            if !cmd {
                self.file_sel.clear();
            }
            for x in a.min(i)..=a.max(i) {
                self.file_sel.insert(x);
            }
        } else if cmd {
            if !self.file_sel.remove(&i) {
                self.file_sel.insert(i);
            }
            self.file_anchor = Some(i);
        } else {
            self.file_sel = HashSet::from([i]);
            self.file_anchor = Some(i);
        }
        cx.notify();
    }

    fn file_context_menu(&mut self, i: usize, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        if !self.file_sel.contains(&i) {
            self.file_sel = HashSet::from([i]);
            self.file_anchor = Some(i);
        }
        let mut ids: Vec<usize> = self.file_sel.iter().copied().collect();
        ids.sort_unstable();
        self.file_menu = Some(FileMenu {
            pos,
            ids,
            open_sub: None,
        });
        self.load_order(cx);
        cx.notify();
    }

    /// Closes the file right-click menu; true if one was open.
    pub fn close_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let was = self.file_menu.take().is_some();
        if was {
            cx.notify();
        }
        was
    }

    fn file_menu_action(&mut self, a: super::menus::FileMenuAction, cx: &mut Context<Self>) {
        use super::menus::FileMenuAction as F;
        let Some(ids) = self.file_menu.take().map(|m| m.ids) else {
            return;
        };
        match a {
            F::Order(patch) => self.set_order(patch, cx),
            F::Include(inc) => {
                let Some(d) = &self.details else { return };
                let mut included: HashSet<usize> = d
                    .files
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.included)
                    .map(|(i, _)| i)
                    .collect();
                for id in ids {
                    if inc {
                        included.insert(id);
                    } else {
                        included.remove(&id);
                    }
                }
                self.set_included(included, cx);
            }
            F::Rename(i) => {
                if let Some(name) = self.details.as_ref().and_then(|d| d.files.get(i)).map(|f| f.name.clone()) {
                    self.rename_file = Some(i);
                    self.rename_input.update(cx, |inp, cx| inp.set_text(name, cx));
                }
            }
        }
        cx.notify();
    }

    fn render_file_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let m = self.file_menu.as_ref()?;
        let d = self.details.as_ref()?;
        let names: Vec<String> = m.ids.iter().filter_map(|i| d.files.get(*i)).map(|f| f.name.clone()).collect();
        let all_included = m.ids.iter().all(|i| d.files.get(*i).is_some_and(|f| f.included));
        let entries = super::menus::file_menu(&m.ids, &names, all_included, self.order.as_ref());
        Some(super::context_menu::render_menu(
            m.pos,
            window.viewport_size(),
            &entries,
            m.open_sub,
            super::context_menu::MenuHost {
                on_action: |this: &mut Self, a, cx| this.file_menu_action(a, cx),
                on_sub: |this: &mut Self, sub, cx| {
                    if let Some(m) = &mut this.file_menu
                        && m.open_sub != sub
                    {
                        m.open_sub = sub;
                        cx.notify();
                    }
                },
                on_close: |this: &mut Self, cx| {
                    if this.file_menu.take().is_some() {
                        cx.notify();
                    }
                },
            },
            cx,
        ))
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
            Tab::Files => {
                self.load_details(cx);
                self.load_order(cx);
            }
            Tab::Peers => self.load_peers(cx),
            Tab::Events => self.load_events(cx),
            Tab::Overview => {}
            Tab::Rules => {
                self.load_rules(true, cx);
                self.load_order(cx);
            }
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
                            "{}% ({}/{})",
                            if s.total_bytes == 0 && !s.finished {
                                "0.0".to_owned()
                            } else {
                                crate::format::format_progress(s.progress_bytes, s.total_bytes, 1)
                            },
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
            .child(lv("Category", super::category::details_text(&t)))
            .child(lv("Playlist", playlist))
            .when_some(hooks, |d, h| d.child(lv("On completion", h)))
            .when_some(s.error.clone(), |d, e| {
                d.child(div().text_color(theme::error()).child(e))
            })
            .when_some(damage_short_text(&t), |d, e| {
                d.child(div().text_color(damage_text_color(&e)).child(e))
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
                                    .map(|t| save_folder_of(&t.output_folder, t.name.as_deref()))
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
                                        // The folder to put it in: a multi-file torrent keeps
                                        // its own `<dest>/<TorrentName>` folder.
                                        let fut = p.client.relocate(id, &dest, p.move_copy, true);
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
            .when_some(self.order.as_ref().map(|o| o.summary.clone()), |d, s| {
                d.child(div().text_xs().text_color(theme::text_muted()).child(format!(
                    "Download order: {s}. Right-click files (ctrl/shift-click to pick several) to change per-file order."
                )))
            })
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
                        let badge = self.order.as_ref().and_then(|o| o.files.iter().find(|x| x.id == i)).and_then(|x| {
                            let mut parts = Vec::new();
                            match x.override_.get("sequential").and_then(Value::as_bool) {
                                Some(true) => parts.push("sequential"),
                                Some(false) => parts.push("not sequential"),
                                None => {}
                            }
                            match x.override_.get("first_last_first").and_then(Value::as_bool) {
                                Some(true) => parts.push("first+last first"),
                                Some(false) => parts.push("no first+last"),
                                None => {}
                            }
                            (!parts.is_empty()).then(|| parts.join(", "))
                        });
                        let highlighted = self.file_sel.contains(&i);
                        div()
                            .id(("file-row", i))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .h(px(24.))
                            .px_1()
                            .rounded_sm()
                            .when(highlighted, |d| d.bg(theme::selected()))
                            .on_click(cx.listener(move |p, ev: &gpui::ClickEvent, _, cx| {
                                p.file_clicked(i, ev.modifiers(), cx)
                            }))
                            .on_mouse_down(
                                gpui::MouseButton::Right,
                                cx.listener(move |p, ev: &gpui::MouseDownEvent, _, cx| {
                                    p.file_context_menu(i, ev.position, cx)
                                }),
                            )
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
                            .when_some(badge, |d, b| {
                                d.child(div().text_xs().text_color(theme::primary()).child(b))
                            })
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
                                    .child(format!("{}%", crate::format::format_progress(done, f.length, 0))),
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

    fn render_rules(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(v) = self.rules.clone() else {
            return div()
                .text_sm()
                .text_color(theme::text_muted())
                .child("Loading…")
                .into_any_element();
        };
        let c = &v.counters;
        let muted = |t: String| div().text_xs().text_color(theme::text_muted()).child(t);
        let mut body = div().flex().flex_col().gap_2().text_sm();
        // Status (server computed: "stops seeding in 2h 10m / 1.2 GB left" etc.).
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme::border())
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Status"))
                .when(v.status.is_empty(), |d| d.child(muted("No automatic rule applies.".into())))
                .children(v.status.iter().map(|s| div().child(format!("• {s}"))))
                .child(muted(format!(
                    "Seeding time {} · uploaded {} · ratio {:.2} · no progress for {}",
                    crate::format::format_duration(c.seeding_secs),
                    format_bytes(c.uploaded_total),
                    v.ratio,
                    crate::format::format_duration(c.idle_secs)
                ))),
        );
        for w in &v.warnings {
            body = body.child(div().text_xs().text_color(theme::error()).child(w.clone()));
        }
        for (si, (section, title)) in RULE_SECTIONS.iter().enumerate() {
            let section: &'static str = section;
            let draft = self.rules_draft.get(section).and_then(Value::as_object).cloned();
            let overridden = draft.is_some();
            let shown = draft
                .clone()
                .or_else(|| v.global.get(section).and_then(Value::as_object).cloned())
                .unwrap_or_default();
            let enabled = shown.get("enabled").and_then(Value::as_bool).unwrap_or(false);
            let mut card = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme::border())
                .child(
                    widgets::checkbox(
                        ("rule-ovr", si),
                        format!(
                            "{title}: {}",
                            if overridden { "custom for this torrent" } else { "global default" }
                        ),
                        overridden,
                        true,
                    )
                    .on_click(cx.listener(move |p, _, _, cx| p.toggle_rule_override(section, cx))),
                );
            if !overridden {
                card = card.child(muted(summarize_rule(section, &shown)));
            } else {
                card = card.child(
                    widgets::checkbox(("rule-en", si), "Enabled", enabled, true).on_click(
                        cx.listener(move |p, _, _, cx| {
                            p.set_rule_field(section, "enabled", Value::Bool(!enabled), cx)
                        }),
                    ),
                );
                for ri in self.rule_inputs.iter().filter(|r| r.path.starts_with(section)) {
                    let label = RULE_FIELDS.iter().find(|f| f.0 == ri.path).map(|f| f.2).unwrap_or("");
                    card = card.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(170.)).text_xs().child(label))
                            .child(div().w(px(200.)).child(ri.input.clone())),
                    );
                }
                let key = if section == "speed_window" { "then" } else { "action" };
                let current = shown.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
                let opts = rule_actions(section);
                let last = opts.len() - 1;
                card = card.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(170.)).text_xs().child("Then"))
                        .child(div().flex().flex_row().children(opts.iter().enumerate().map(
                            |(k, (val, lab))| {
                                let val: &'static str = val;
                                widgets::segment(("rule-act", si * 10 + k), *lab, current == val)
                                    .when(k == 0, |d| d.rounded_l_md())
                                    .when(k == last, |d| d.rounded_r_md())
                                    .on_click(cx.listener(move |p, _, _, cx| {
                                        p.set_rule_field(section, key, Value::String(val.into()), cx)
                                    }))
                            },
                        ))),
                );
            }
            body = body.child(card);
        }
        body = body.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    widgets::primary_button("rules-save", "Save rules", true)
                        .on_click(cx.listener(|p, _, _, cx| p.save_rules(false, cx))),
                )
                .child(
                    widgets::button("rules-reset", "Use global defaults", true)
                        .on_click(cx.listener(|p, _, _, cx| p.save_rules(true, cx))),
                )
                .when(self.rules_dirty, |d| d.child(muted("unsaved changes".into()))),
        );
        // Download order (torrent-wide; per-file in the Files tab).
        if let Some(o) = self.order.clone() {
            let mut card = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme::border())
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Download order"))
                .child(muted(format!("Current: {}", o.summary)));
            let tri_row = |card: gpui::Div, idx: usize, key: &'static str, label: &'static str, cx: &mut Context<Self>| {
                let cur = o.torrent.get(key).and_then(Value::as_bool);
                let g = o.global.get(key).and_then(Value::as_bool).unwrap_or(false);
                let opts: [(&str, Option<bool>); 3] = [
                    (if g { "Default (on)" } else { "Default (off)" }, None),
                    ("On", Some(true)),
                    ("Off", Some(false)),
                ];
                card.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(230.)).text_xs().child(label))
                        .child(div().flex().flex_row().children(opts.into_iter().enumerate().map(
                            |(k, (lab, val))| {
                                widgets::segment(("ord", idx * 10 + k), lab.to_owned(), cur == val)
                                    .when(k == 0, |d| d.rounded_l_md())
                                    .when(k == 2, |d| d.rounded_r_md())
                                    .on_click(cx.listener(move |p, _, _, cx| {
                                        p.set_order(serde_json::json!({ key: val }), cx)
                                    }))
                            },
                        ))),
                )
            };
            card = tri_row(card, 0, "sequential_files", "Sequential file download", cx);
            let cur_order = o.torrent.get("file_order").and_then(Value::as_str).map(str::to_owned);
            let mut order_opts: Vec<(String, Option<&'static str>)> = vec![("Default".into(), None)];
            order_opts.extend(super::menus::FILE_ORDERS.iter().map(|(k, l)| (l.to_string(), Some(*k))));
            let last = order_opts.len() - 1;
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(230.)).text_xs().child("File order"))
                    .child(div().flex().flex_row().children(order_opts.into_iter().enumerate().map(
                        |(k, (lab, val))| {
                            widgets::segment(("ord-fo", k), lab, cur_order.as_deref() == val)
                                .when(k == 0, |d| d.rounded_l_md())
                                .when(k == last, |d| d.rounded_r_md())
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.set_order(serde_json::json!({ "file_order": val }), cx)
                                }))
                        },
                    ))),
            );
            card = tri_row(card, 1, "sequential", "Sequential download (all files)", cx);
            card = tri_row(card, 2, "first_last_first", "First and last pieces first (all files)", cx);
            card = card.child(muted("Per-file settings (Files tab, right-click) override these.".into()));
            body = body.child(card);
        }
        body.into_any_element()
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

    fn render_events(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let open_all = self.torrent.as_ref().map(|t| {
            let (hash, name) = (t.info_hash.clone(), t.name.clone().unwrap_or_default());
            div().flex().flex_row().pb_2().child(
                widgets::button("d-events-all", "Open in Events view", true).on_click(cx.listener(
                    move |_, _, _, cx| {
                        cx.emit(DetailsEvent::OpenEvents(hash.clone(), name.clone()))
                    },
                )),
            )
        });
        let list = match &self.events {
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
        };
        div()
            .flex()
            .flex_col()
            .children(open_all)
            .child(list)
            .into_any_element()
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
        "disk_full" => "Disk full",
        "disk_space_available" => "Disk space OK",
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = [
            (Tab::Overview, "Overview"),
            (Tab::Files, "Files"),
            (Tab::Peers, "Peers"),
            (Tab::Events, "Events"),
            (Tab::Rules, "Rules"),
        ];
        let body = match self.tab {
            Tab::Overview => self.render_overview(cx),
            Tab::Files => self.render_files(cx),
            Tab::Peers => self.render_peers(),
            Tab::Events => self.render_events(cx),
            Tab::Rules => self.render_rules(cx),
        };
        let file_menu = self.render_file_menu(window, cx);
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
            .children(file_menu)
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
    fn rule_values() {
        assert_eq!(rule_value(RuleKind::DurationReq, "x", "").unwrap(), None);
        assert_eq!(rule_value(RuleKind::Duration, "x", "").unwrap(), Some(Value::Null));
        assert_eq!(rule_value(RuleKind::Duration, "x", "2h").unwrap(), Some(serde_json::json!(7200)));
        assert_eq!(rule_value(RuleKind::Size, "x", "1 GB").unwrap(), Some(serde_json::json!(1u64 << 30)));
        assert!(rule_value(RuleKind::Float, "x", "abc").is_err());
        assert_eq!(rule_text(RuleKind::Duration, Some(&serde_json::json!(90))), "1m 30s");
        let r: serde_json::Map<String, Value> = serde_json::from_value(serde_json::json!(
            {"enabled": true, "max_seed_secs": 3600, "max_ratio": 2.0, "action": "pause"}
        )).unwrap();
        assert_eq!(summarize_rule("seeding", &r), "1h / ratio 2 → pause");
    }

    #[test]
    fn event_time() {
        assert_eq!(format_event_time("garbage"), "garbage");
        let s = format_event_time("2020-01-02T03:04:05Z");
        assert!(s.starts_with("Jan "), "{s}");
    }
}

/// The folder a torrent is saved in: the parent of its own `<name>` folder, if it has one
/// (what the Move form starts with; the server puts a multi-file torrent in
/// `<folder>/<name>`).
pub(crate) fn save_folder_of(output_folder: &str, name: Option<&str>) -> String {
    let trimmed = output_folder.trim_end_matches(['/', '\\']);
    if let (Some(name), Some(cut)) = (name, trimmed.rfind(['/', '\\']))
        && cut > 0
        && &trimmed[cut + 1..] == name
    {
        return trimmed[..cut].to_owned();
    }
    output_folder.to_owned()
}

#[cfg(test)]
mod save_folder_tests {
    use super::save_folder_of;

    #[test]
    fn move_form_starts_in_the_save_folder() {
        assert_eq!(save_folder_of("/dl/Show S01", Some("Show S01")), "/dl");
        assert_eq!(save_folder_of("/dl/Show S01/", Some("Show S01")), "/dl");
        assert_eq!(save_folder_of("D:\\dl\\Show", Some("Show")), "D:\\dl");
        assert_eq!(save_folder_of("/done", Some("Show")), "/done");
        assert_eq!(save_folder_of("/Show", Some("Show")), "/Show");
        assert_eq!(save_folder_of("/dl/x", None), "/dl/x");
    }
}
