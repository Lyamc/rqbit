//! Entries of the right-click menus (pure, unit tested). Mirrors the web
//! UI's `TorrentContextMenu.tsx` and `helper/downloadOrderMenu.ts`.

use serde_json::{Value, json};

use super::TorrentAction;
use super::context_menu::MenuEntry;
use crate::api::{DownloadOrderView, TorrentState};

#[derive(Clone, Debug, PartialEq)]
pub enum RowMenuAction {
    Details,
    Act(TorrentAction),
    Queue(&'static str),
    /// `POST /download_order` patch, applied to every selected torrent.
    Order(Value),
    Move,
    SetCategory,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FileMenuAction {
    /// Patch for the right-clicked files.
    Order(Value),
    Include(bool),
    Rename(usize),
}

pub const FILE_ORDERS: [(&str, &str); 4] = [
    ("name", "By name"),
    ("torrent", "Torrent order"),
    ("smallest_first", "Smallest first"),
    ("largest_first", "Largest first"),
];

/// Per selected torrent: state, and whether Fix errors applies.
pub struct RowInfo {
    pub state: Option<TorrentState>,
    pub fixable: bool,
}

fn tri<A: Clone>(
    current: Option<Option<bool>>,
    effective: Option<bool>,
    default_label: &str,
    make: impl Fn(Value) -> A,
) -> Vec<MenuEntry<A>> {
    let is = |v: Option<bool>| current == Some(v);
    let hint = effective.map(|e| if e { "on" } else { "off" });
    let mut d = MenuEntry::check(default_label.to_owned(), is(None), make(Value::Null));
    if let Some(h) = hint {
        d = d.hint(h);
    }
    vec![
        MenuEntry::check("On", is(Some(true)), make(json!(true))),
        MenuEntry::check("Off", is(Some(false)), make(json!(false))),
        d,
    ]
}

/// Current torrent-level override of `key` (Some(None) = inherit) when known.
fn torrent_value(view: Option<&DownloadOrderView>, key: &str) -> Option<Option<bool>> {
    view.map(|v| v.torrent.get(key).and_then(Value::as_bool))
}

pub fn torrent_order_entries<A: Clone>(
    view: Option<&DownloadOrderView>,
    make: impl Fn(Value) -> A + Copy,
) -> Vec<MenuEntry<A>> {
    let g = |k: &str| view.and_then(|v| v.global.get(k)).and_then(Value::as_bool);
    let cur_order = view.map(|v| v.torrent.get("file_order").and_then(Value::as_str).map(str::to_owned));
    let global_order = view
        .and_then(|v| v.global.get("file_order"))
        .and_then(Value::as_str)
        .and_then(|k| FILE_ORDERS.iter().find(|(x, _)| *x == k))
        .map(|(_, l)| l.to_lowercase());
    let mut order_items: Vec<MenuEntry<A>> = FILE_ORDERS
        .iter()
        .map(|(k, l)| {
            MenuEntry::check(
                *l,
                cur_order.as_ref().is_some_and(|c| c.as_deref() == Some(*k)),
                make(json!({ "file_order": k })),
            )
        })
        .collect();
    let mut def = MenuEntry::check(
        "Use default",
        cur_order.as_ref().is_some_and(|c| c.is_none()),
        make(json!({ "file_order": null })),
    );
    if let Some(h) = global_order {
        def = def.hint(h);
    }
    order_items.push(def);
    let field = |key: &'static str| move |v: Value| make(json!({ key: v }));
    vec![
        MenuEntry::sub(
            "Sequential file download",
            true,
            tri(torrent_value(view, "sequential_files"), g("sequential_files"), "Use default", field("sequential_files")),
        ),
        MenuEntry::sub("File order", true, order_items),
        MenuEntry::sub(
            "Sequential download (all files)",
            true,
            tri(torrent_value(view, "sequential"), g("sequential"), "Use default", field("sequential")),
        ),
        MenuEntry::sub(
            "First and last pieces first (all files)",
            true,
            tri(torrent_value(view, "first_last_first"), g("first_last_first"), "Use default", field("first_last_first")),
        ),
        MenuEntry::item("Reset download order", make(json!({ "reset": true }))),
    ]
}

/// Full torrent-row menu for `n` selected torrents.
pub fn torrent_menu(
    rows: &[RowInfo],
    order: Option<&DownloadOrderView>,
    queue_supported: bool,
) -> Vec<MenuEntry<RowMenuAction>> {
    use RowMenuAction as R;
    let n = rows.len();
    let single = n == 1;
    let all = |s: TorrentState| !rows.is_empty() && rows.iter().all(|r| r.state == Some(s));
    let count = |l: &str| if n > 1 { format!("{l} ({n})") } else { l.to_owned() };
    let mut order_sub = torrent_order_entries(order, R::Order);
    // Only one torrent's settings can be shown; with several, choices apply to all.
    if !single {
        order_sub.insert(0, MenuEntry::disabled(format!("Applies to all {n} torrents")));
        order_sub.insert(1, MenuEntry::Sep);
    }
    vec![
        MenuEntry::when_enabled("Open details", single, R::Details),
        MenuEntry::Sep,
        MenuEntry::when_enabled(count("Resume"), !all(TorrentState::Live), R::Act(TorrentAction::Start)),
        MenuEntry::when_enabled(count("Pause"), !all(TorrentState::Paused), R::Act(TorrentAction::Pause)),
        MenuEntry::item(count("Restart"), R::Act(TorrentAction::Restart)),
        MenuEntry::when_enabled(
            count("Fix errors"),
            rows.iter().any(|r| r.fixable),
            R::Act(TorrentAction::FixErrors),
        ),
        MenuEntry::item(count("Force recheck"), R::Act(TorrentAction::Recheck)),
        MenuEntry::Sep,
        MenuEntry::sub(
            "Queue",
            queue_supported,
            vec![
                MenuEntry::item("Move to top", R::Queue("top")),
                MenuEntry::item("Move up", R::Queue("up")),
                MenuEntry::item("Move down", R::Queue("down")),
                MenuEntry::item("Move to bottom", R::Queue("bottom")),
            ],
        ),
        MenuEntry::sub("Download order", true, order_sub),
        MenuEntry::when_enabled(
            if single { "Move files…".to_owned() } else { "Move files… (one torrent at a time)".to_owned() },
            single,
            R::Move,
        ),
        MenuEntry::item(
            if n > 1 { format!("Set category ({n})…") } else { "Set category…".to_owned() },
            R::SetCategory,
        ),
        MenuEntry::Sep,
        MenuEntry::item(
            if single { "Remove…".to_owned() } else { format!("Remove {n} torrents…") },
            R::Remove,
        )
        .danger(),
    ]
}

/// File-row menu for the highlighted `ids`.
pub fn file_menu(
    ids: &[usize],
    names: &[String],
    all_included: bool,
    order: Option<&DownloadOrderView>,
) -> Vec<MenuEntry<FileMenuAction>> {
    use FileMenuAction as F;
    let files: Vec<_> = order
        .map(|o| o.files.iter().filter(|f| ids.contains(&f.id)).collect())
        .unwrap_or_default();
    let known = !ids.is_empty() && files.len() == ids.len();
    let same = |key: &str| -> Option<Option<bool>> {
        if !known {
            return None;
        }
        let vals: Vec<Option<bool>> = files.iter().map(|f| f.override_.get(key).and_then(Value::as_bool)).collect();
        vals.iter().all(|v| *v == vals[0]).then(|| vals[0])
    };
    let eff = |k: &str| order.and_then(|o| o.effective.get(k)).and_then(Value::as_bool);
    let ids_v = json!(ids);
    let patch = move |key: &'static str| {
        let ids_v = ids_v.clone();
        move |v: Value| F::Order(json!({ "files": [{ "ids": ids_v.clone(), key: v }] }))
    };
    let title = if ids.len() == 1 {
        names.first().cloned().unwrap_or_else(|| format!("File #{}", ids[0]))
    } else {
        format!("{} files", ids.len())
    };
    vec![
        MenuEntry::disabled(title),
        MenuEntry::Sep,
        MenuEntry::sub(
            "Sequential download",
            true,
            tri(same("sequential"), eff("sequential"), "Same as torrent", patch("sequential")),
        ),
        MenuEntry::sub(
            "Download first and last pieces first",
            true,
            tri(same("first_last_first"), eff("first_last_first"), "Same as torrent", patch("first_last_first")),
        ),
        MenuEntry::item(
            "Reset download order for these files",
            F::Order(json!({ "files": [{ "ids": ids, "sequential": null, "first_last_first": null }] })),
        ),
        MenuEntry::Sep,
        MenuEntry::item(
            if all_included { "Don't download" } else { "Download" },
            F::Include(!all_included),
        ),
        MenuEntry::when_enabled("Rename…", ids.len() == 1, F::Rename(ids.first().copied().unwrap_or(0))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels<A: Clone>(e: &[MenuEntry<A>]) -> Vec<String> {
        e.iter().map(|x| x.describe()).collect()
    }

    fn find_sub<'a, A: Clone>(e: &'a [MenuEntry<A>], label: &str) -> &'a [MenuEntry<A>] {
        e.iter()
            .find_map(|x| match x {
                MenuEntry::Sub { label: l, items, .. } if l.as_ref() == label => Some(items.as_slice()),
                _ => None,
            })
            .expect("submenu")
    }

    fn action<A: Clone>(e: &[MenuEntry<A>], label: &str) -> Option<A> {
        e.iter().find_map(|x| match x {
            MenuEntry::Item { label: l, action, .. } if l.as_ref() == label => action.clone(),
            _ => None,
        })
    }

    #[test]
    fn torrent_menu_has_all_actions() {
        let rows = [
            RowInfo { state: Some(TorrentState::Paused), fixable: false },
            RowInfo { state: Some(TorrentState::Paused), fixable: false },
        ];
        let m = torrent_menu(&rows, None, true);
        let l = labels(&m);
        for want in [
            "Open details (disabled)",
            "Resume (2)",
            "Pause (2) (disabled)",
            "Restart (2)",
            "Fix errors (2) (disabled)",
            "Force recheck (2)",
            "Queue >",
            "Download order >",
            "Set category (2)…",
            "Remove 2 torrents…",
        ] {
            assert!(l.iter().any(|x| x == want), "missing {want}: {l:?}");
        }
        let sub = find_sub(&m, "Download order");
        assert!(labels(sub)[0].starts_with("Applies to all 2"));
    }

    #[test]
    fn order_submenu_marks_and_patches() {
        let view: DownloadOrderView = serde_json::from_value(json!({
            "global": {"sequential_files": true, "file_order": "name", "sequential": true, "first_last_first": true},
            "torrent": {"file_order": "smallest_first", "sequential": false},
            "effective": {"sequential_files": true, "file_order": "smallest_first", "sequential": false, "first_last_first": true},
            "files": [{"id": 0, "name": "a", "sequential": true, "first_last_first": true, "override": {"sequential": true}},
                      {"id": 1, "name": "b", "sequential": false, "first_last_first": true, "override": {}}],
            "summary": "x"
        }))
        .unwrap();
        let m = torrent_menu(&[RowInfo { state: None, fixable: false }], Some(&view), true);
        let order = find_sub(find_sub(&m, "Download order"), "File order");
        assert!(labels(order).contains(&"[x] Smallest first".to_owned()));
        let seq = find_sub(find_sub(&m, "Download order"), "Sequential download (all files)");
        assert!(labels(seq).contains(&"[x] Off".to_owned()));
        assert_eq!(
            action(seq, "Use default"),
            Some(RowMenuAction::Order(json!({"sequential": null})))
        );
        let sf = find_sub(find_sub(&m, "Download order"), "Sequential file download");
        assert!(labels(sf).contains(&"[x] Use default".to_owned()));

        // Files: file 0 overrides sequential, file 1 doesn't → mixed, nothing checked.
        let fm = file_menu(&[0, 1], &[], true, Some(&view));
        let s = find_sub(&fm, "Sequential download");
        assert!(labels(s).iter().all(|x| !x.starts_with("[x]")), "{:?}", labels(s));
        assert_eq!(
            action(s, "On"),
            Some(FileMenuAction::Order(json!({"files": [{"ids": [0, 1], "sequential": true}]})))
        );
        let fm = file_menu(&[0], &["a".into()], true, Some(&view));
        assert!(labels(find_sub(&fm, "Sequential download")).contains(&"[x] On".to_owned()));
        assert_eq!(action(&fm, "Don't download"), Some(FileMenuAction::Include(false)));
    }
}
