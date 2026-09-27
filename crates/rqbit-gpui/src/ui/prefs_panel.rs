//! Preferences, backed by the same endpoints as the web UI's Configure
//! dialog (`ConfigModal.tsx`): `GET/POST /torrents/limits`,
//! `GET/POST /torrents/preferences`, `GET /admin` + `POST /admin/config`.
//! Same tabs: Speed, Connection, BitTorrent, Downloads, Organize,
//! Completion, Interface, Web UI / Admin.
//!
//! Saving only sends what changed: limits if edited, the full preferences
//! object (as read from the server, unknown fields preserved) if any
//! preference changed, and an admin.json patch with just the edited keys.
//!
//! Read-only here (edit in the web UI): the completion action pipeline
//! editor, and the Admin tab's HTTP listen address / basic auth / reload /
//! restart.

use gpui::{Context, Entity, EventEmitter, SharedString, Window, div, prelude::*, px};
use serde_json::{Map, Value, json};

use super::text_input::TextInput;
use super::{theme, widgets};
use crate::api::{AdminStatus, ApiClient, LimitsConfig};

pub enum PrefsPanelEvent {
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Speed,
    Connection,
    BitTorrent,
    Downloads,
    Organize,
    Completion,
    Automation,
    Interface,
    Admin,
}

const TABS: [(Tab, &str); 9] = [
    (Tab::Speed, "Speed"),
    (Tab::Connection, "Connection"),
    (Tab::BitTorrent, "BitTorrent"),
    (Tab::Downloads, "Downloads"),
    (Tab::Organize, "Organize"),
    (Tab::Completion, "Completion"),
    (Tab::Automation, "Automation"),
    (Tab::Interface, "Interface"),
    (Tab::Admin, "Web UI / Admin"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Limits,
    Prefs,
    Admin,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Free text; empty -> null.
    Text,
    /// Optional non-negative integer; empty (or 0) -> null / clear.
    OptInt,
    /// Required integer; empty keeps the current value.
    Int,
    /// Optional duration ("2h 30m", bare number = minutes); empty -> null.
    Duration,
    /// Required duration; empty keeps the current value.
    DurationReq,
    /// Optional size ("1.5 GB"); empty -> null.
    Size,
    /// Optional positive decimal (ratio); empty -> null.
    Float,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tri {
    Default,
    On,
    Off,
}

struct InputField {
    target: Target,
    key: &'static str,
    /// admin.json: `clear_<key>` flag used for "back to startup default".
    clear_key: Option<&'static str>,
    kind: Kind,
    label: &'static str,
    help: &'static str,
    input: Entity<TextInput>,
    initial: String,
}

struct BoolField {
    key: &'static str,
    label: &'static str,
    help: &'static str,
    value: bool,
    initial: bool,
}

/// One of a few string values (segmented buttons), stored in preferences.
struct ChoiceField {
    key: &'static str,
    label: &'static str,
    help: &'static str,
    options: &'static [(&'static str, &'static str)],
    value: &'static str,
    initial: &'static str,
}

const WHEN_ADDED: &[(&str, &str)] = &[("start", "Start immediately"), ("paused", "Add paused")];
const COMPLETE_ACTIONS: &[(&str, &str)] = &[("keep", "Keep files"), ("delete", "Delete files")];
const INCOMPLETE_ACTIONS: &[(&str, &str)] = &[
    ("keep", "Keep files"),
    ("delete", "Delete all"),
    ("finish", "Finish what's done"),
];
const STALLED_ACTIONS: &[(&str, &str)] = &[
    ("flag", "Flag"),
    ("pause", "Pause"),
    ("remove_keep", "Remove (keep)"),
    ("remove_finish", "Remove (finish)"),
    ("remove_delete", "Remove + delete"),
    ("remove_policy", "Remove per policy"),
];
const SEEDING_ACTIONS: &[(&str, &str)] = &[
    ("pause", "Pause"),
    ("remove_keep", "Remove (keep files)"),
    ("remove_policy", "Remove per policy"),
];
const WINDOW_THEN: &[(&str, &str)] = &[("cap", "Cap upload rate"), ("stop", "Stop seeding")];
const FILE_ORDERS: &[(&str, &str)] = &[
    ("name", "By name"),
    ("torrent", "Torrent order"),
    ("smallest_first", "Smallest first"),
    ("largest_first", "Largest first"),
];

/// Value at a dotted path ("rules.stalled.enabled").
fn get_path<'a>(m: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut cur = m.get(parts.next()?)?;
    for p in parts {
        cur = cur.get(p)?;
    }
    Some(cur)
}

/// Sets a dotted path, creating (or replacing non-object) parents.
fn set_path(m: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            m.insert(path.to_owned(), value);
        }
        Some((head, rest)) => {
            let e = m
                .entry(head.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !e.is_object() {
                *e = Value::Object(Map::new());
            }
            set_path(e.as_object_mut().expect("object"), rest, value);
        }
    }
}

/// Maps a stored string to the matching `&'static` option value.
fn choice_value(
    options: &'static [(&'static str, &'static str)],
    v: Option<&str>,
    default: &'static str,
) -> &'static str {
    options
        .iter()
        .find(|(k, _)| Some(*k) == v)
        .map(|(k, _)| *k)
        .unwrap_or(default)
}

/// Whether a rule action can delete files (mirrors `RuleAction::may_delete`).
fn rule_may_delete(action: &str, complete: bool, policy: (&str, &str)) -> bool {
    let p = match action {
        "remove_keep" => ("keep", "keep"),
        "remove_delete" => ("delete", "delete"),
        "remove_finish" => ("keep", "finish"),
        "remove_policy" => policy,
        _ => return false,
    };
    if complete { p.0 == "delete" } else { p.1 != "keep" }
}

struct TriField {
    key: &'static str,
    clear_key: &'static str,
    /// Stored value is the opposite of "enabled" (e.g. disable_dht).
    inverted: bool,
    label: &'static str,
    help: &'static str,
    value: Tri,
    initial: Tri,
}

enum Row {
    Heading(&'static str),
    Note(SharedString),
    Input(usize),
    Bool(usize),
    Tri(usize),
    Choice(usize),
    Info(&'static str, String),
    /// Live warnings about settings that can delete files.
    Warnings,
    /// Basic auth editor (admin.json).
    Auth,
    /// Reload preferences.json / restart the process.
    AdminOps,
    /// Default app for magnet links / .torrent files (this computer or browser).
    Handlers,
}

struct Loaded {
    limits: LimitsConfig,
    prefs: Map<String, Value>,
    inputs: Vec<InputField>,
    bools: Vec<BoolField>,
    tris: Vec<TriField>,
    choices: Vec<ChoiceField>,
    rows: Vec<(Tab, Row)>,
    auth: AuthFields,
    restart_supported: bool,
}

struct AuthFields {
    enabled: bool,
    initial_enabled: bool,
    user: Entity<TextInput>,
    initial_user: String,
    password: Entity<TextInput>,
    password_set: bool,
}

/// admin.json keys for a basic auth change (web UI AdminTab): nothing if
/// unchanged; the password only when typed.
fn auth_patch(
    enabled: bool,
    initial_enabled: bool,
    user: &str,
    initial_user: &str,
    password: &str,
) -> Map<String, Value> {
    let mut m = Map::new();
    let user = user.trim();
    if enabled == initial_enabled && (!enabled || (user == initial_user && password.is_empty())) {
        return m;
    }
    m.insert("basic_auth_enabled".into(), json!(enabled));
    if enabled {
        m.insert("basic_auth_user".into(), json!(user));
        if !password.is_empty() {
            m.insert("basic_auth_password".into(), json!(password));
        }
    }
    m
}

pub struct PrefsPanel {
    client: ApiClient,
    tab: Tab,
    loading: bool,
    saving: bool,
    error: Option<String>,
    message: Option<String>,
    loaded: Option<Loaded>,
    confirm_restart: bool,
    /// Default-handler status line and the last register/unregister result.
    handler_status: Option<String>,
    handler_msg: Option<String>,
}

impl EventEmitter<PrefsPanelEvent> for PrefsPanel {}

fn value_to_text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}

fn tri_from(v: Option<&Value>, inverted: bool) -> Tri {
    match v.and_then(Value::as_bool) {
        None => Tri::Default,
        Some(b) => {
            if b != inverted {
                Tri::On
            } else {
                Tri::Off
            }
        }
    }
}

const ORGANIZE_FOLDERS: [(&str, &str); 9] = [
    ("anime", "Anime"),
    ("tv", "TV"),
    ("movie", "Movies"),
    ("game", "Games"),
    ("porn", "Adult"),
    ("music", "Music"),
    ("book", "Books"),
    ("software", "Software"),
    ("other", "Other"),
];

/// Parses an integer field. Ok(None) = empty.
fn parse_int(label: &str, text: &str) -> Result<Option<u64>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<u64>()
        .map(Some)
        .map_err(|_| format!("{label}: \"{t}\" is not a whole number"))
}

/// What a save would send. Pure so it can be unit tested.
#[derive(Debug, Default, PartialEq)]
struct SavePlan {
    limits: Option<LimitsConfig>,
    prefs: Option<Map<String, Value>>,
    admin: Option<Map<String, Value>>,
}

struct FieldValue<'a> {
    target: Target,
    key: &'a str,
    clear_key: Option<&'a str>,
    kind: Kind,
    label: &'a str,
    text: String,
    initial: String,
}

fn plan_save(
    limits: &LimitsConfig,
    prefs: &Map<String, Value>,
    inputs: &[FieldValue<'_>],
    bools: &[(&str, bool, bool)],
    tris: &[(&str, &str, bool, Tri, Tri)],
) -> Result<SavePlan, Vec<String>> {
    let mut errors = Vec::new();
    let mut new_limits = limits.clone();
    let mut new_prefs = prefs.clone();
    let mut prefs_changed = false;
    let mut admin = Map::new();

    for f in inputs {
        if f.text == f.initial {
            continue;
        }
        let text = f.text.trim();
        match f.target {
            Target::Limits => match parse_int(f.label, text) {
                Ok(v) => {
                    let v = v.filter(|v| *v > 0);
                    if v.is_some_and(|v| v > u32::MAX as u64) {
                        errors.push(format!("{}: too large", f.label));
                        continue;
                    }
                    match f.key {
                        "download_bps" => new_limits.download_bps = v,
                        _ => new_limits.upload_bps = v,
                    }
                }
                Err(e) => errors.push(e),
            },
            Target::Prefs => {
                let value = match f.kind {
                    Kind::Text => {
                        if text.is_empty() {
                            Value::Null
                        } else {
                            Value::String(f.text.clone())
                        }
                    }
                    Kind::OptInt => match parse_int(f.label, text) {
                        Ok(Some(v)) if v > 0 => json!(v),
                        Ok(_) => Value::Null,
                        Err(e) => {
                            errors.push(e);
                            continue;
                        }
                    },
                    Kind::Int => match parse_int(f.label, text) {
                        Ok(Some(v)) => json!(v),
                        Ok(None) => continue,
                        Err(e) => {
                            errors.push(e);
                            continue;
                        }
                    },
                    Kind::Duration | Kind::DurationReq => {
                        if text.is_empty() {
                            if f.kind == Kind::DurationReq {
                                continue;
                            }
                            Value::Null
                        } else {
                            match crate::format::parse_duration(text, 60) {
                                Some(v) => json!(v),
                                None => {
                                    errors.push(format!(
                                        "{}: \"{text}\" is not a duration (e.g. 15m, 2h 30m, 7d)",
                                        f.label
                                    ));
                                    continue;
                                }
                            }
                        }
                    }
                    Kind::Size => {
                        if text.is_empty() {
                            Value::Null
                        } else {
                            match crate::format::parse_size(text) {
                                Some(v) => json!(v),
                                None => {
                                    errors.push(format!(
                                        "{}: \"{text}\" is not a size (e.g. 500 MB, 1.5 GB)",
                                        f.label
                                    ));
                                    continue;
                                }
                            }
                        }
                    }
                    Kind::Float => {
                        if text.is_empty() {
                            Value::Null
                        } else {
                            match text.parse::<f64>() {
                                Ok(v) if v.is_finite() && v > 0.0 => json!(v),
                                Ok(_) => Value::Null,
                                Err(_) => {
                                    errors.push(format!("{}: \"{text}\" is not a number", f.label));
                                    continue;
                                }
                            }
                        }
                    }
                };
                if let Some(folder) = f.key.strip_prefix("auto_organize_folders.") {
                    let obj = new_prefs
                        .entry("auto_organize_folders")
                        .or_insert_with(|| Value::Object(Map::new()));
                    if !obj.is_object() {
                        *obj = Value::Object(Map::new());
                    }
                    let map = obj.as_object_mut().expect("object");
                    for (k, d) in ORGANIZE_FOLDERS {
                        map.entry(k).or_insert_with(|| json!(d));
                    }
                    map.insert(
                        folder.to_owned(),
                        if text.is_empty() {
                            json!(
                                ORGANIZE_FOLDERS
                                    .iter()
                                    .find(|(k, _)| *k == folder)
                                    .map(|(_, d)| *d)
                                    .unwrap_or("")
                            )
                        } else {
                            json!(text)
                        },
                    );
                } else {
                    set_path(&mut new_prefs, f.key, value);
                }
                prefs_changed = true;
            }
            Target::Admin => match f.kind {
                Kind::Text => {
                    admin.insert(
                        f.key.to_owned(),
                        // "" clears (e.g. the listen address); null would
                        // mean "unchanged" to the server.
                        json!(text),
                    );
                }
                _ => match parse_int(f.label, text) {
                    Ok(Some(v)) if v > 0 => {
                        admin.insert(f.key.to_owned(), json!(v));
                    }
                    Ok(_) => {
                        if let Some(c) = f.clear_key {
                            admin.insert(c.to_owned(), json!(true));
                        }
                    }
                    Err(e) => errors.push(e),
                },
            },
        }
    }
    for (key, value, initial) in bools {
        if value != initial {
            set_path(&mut new_prefs, key, json!(value));
            prefs_changed = true;
        }
    }
    for (key, clear_key, inverted, value, initial) in tris {
        if value == initial {
            continue;
        }
        match value {
            Tri::Default => {
                admin.insert((*clear_key).to_owned(), json!(true));
            }
            Tri::On | Tri::Off => {
                let enabled = *value == Tri::On;
                admin.insert((*key).to_owned(), json!(enabled != *inverted));
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(SavePlan {
        limits: (new_limits != *limits).then_some(new_limits),
        prefs: prefs_changed.then_some(new_prefs),
        admin: (!admin.is_empty()).then_some(admin),
    })
}

/// Current (unsaved) settings that can delete files, shown as warnings in
/// the Interface and Automation tabs.
fn delete_warnings(l: &Loaded) -> Vec<String> {
    let choice = |k: &str| l.choices.iter().find(|c| c.key == k).map(|c| c.value);
    let boolv = |k: &str| l.bools.iter().any(|b| b.key == k && b.value);
    let policy = (
        choice("remove_policy.complete").unwrap_or("keep"),
        choice("remove_policy.incomplete").unwrap_or("keep"),
    );
    warnings_for(
        policy,
        boolv("confirm_remove"),
        boolv("rules.stalled.enabled").then(|| choice("rules.stalled.action").unwrap_or("pause")),
        boolv("rules.seeding.enabled").then(|| choice("rules.seeding.action").unwrap_or("pause")),
    )
}

fn warnings_for(
    policy: (&str, &str),
    confirm: bool,
    stalled: Option<&str>,
    seeding: Option<&str>,
) -> Vec<String> {
    let mut w = Vec::new();
    if !confirm && (policy.0 == "delete" || policy.1 != "keep") {
        w.push("Confirmation is off, but this remove policy can delete files, so the dialog is still shown whenever a removal would delete something.".to_owned());
    }
    if let Some(a) = stalled
        && rule_may_delete(a, false, policy)
    {
        w.push("Stalled-torrent rule: torrents will be removed with file deletion.".to_owned());
    }
    if let Some(a) = seeding
        && rule_may_delete(a, true, policy)
    {
        w.push("Seeding-limit rule: torrents will have their files deleted (remove policy deletes complete torrents).".to_owned());
    }
    w
}

/// Adds changed choice fields to a save plan (the full preferences object,
/// unknown fields preserved, like `plan_save`).
fn apply_choices(plan: &mut SavePlan, prefs: &Map<String, Value>, choices: &[(&str, &str, &str)]) {
    for (key, value, initial) in choices {
        if value != initial {
            set_path(plan.prefs.get_or_insert_with(|| prefs.clone()), key, json!(value));
        }
    }
}

impl PrefsPanel {
    pub fn new(client: ApiClient, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            client,
            tab: Tab::Speed,
            loading: true,
            saving: false,
            error: None,
            message: None,
            loaded: None,
            confirm_restart: false,
            handler_status: None,
            handler_msg: None,
        };
        this.reload(cx);
        this.refresh_handler_status(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let bg = cx.background_executor().clone();
            let limits = bg.spawn(client.get_limits()).await;
            let prefs = bg.spawn(client.get_preferences()).await;
            let admin = bg.spawn(client.get_admin_status()).await;
            this.update(cx, |p, cx| {
                p.loading = false;
                match (limits, prefs, admin) {
                    (Ok(l), Ok(pr), Ok(a)) => {
                        p.error = None;
                        p.build(l, pr, a, cx);
                    }
                    (l, pr, a) => {
                        let e = l.err().or(pr.err()).or(a.err()).expect("one failed");
                        p.error = Some(format!("Error loading configuration: {e:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn build(
        &mut self,
        limits: LimitsConfig,
        prefs: Map<String, Value>,
        admin: AdminStatus,
        cx: &mut Context<Self>,
    ) {
        let mut inputs: Vec<InputField> = Vec::new();
        let mut bools: Vec<BoolField> = Vec::new();
        let mut tris: Vec<TriField> = Vec::new();
        let mut rows: Vec<(Tab, Row)> = Vec::new();
        let persisted = admin.persisted.clone();

        let mut input = |tab: Tab,
                         target: Target,
                         key: &'static str,
                         clear_key: Option<&'static str>,
                         kind: Kind,
                         label: &'static str,
                         help: &'static str,
                         placeholder: &'static str,
                         rows: &mut Vec<(Tab, Row)>,
                         cx: &mut Context<Self>| {
            let initial = match target {
                Target::Limits => match key {
                    "download_bps" => limits
                        .download_bps
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    _ => limits.upload_bps.map(|v| v.to_string()).unwrap_or_default(),
                },
                Target::Prefs => match key.strip_prefix("auto_organize_folders.") {
                    Some(folder) => prefs
                        .get("auto_organize_folders")
                        .and_then(|o| o.get(folder))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            ORGANIZE_FOLDERS
                                .iter()
                                .find(|(k, _)| *k == folder)
                                .map(|(_, d)| d.to_string())
                                .unwrap_or_default()
                        }),
                    None => {
                        let v = get_path(&prefs, key);
                        match kind {
                            Kind::Duration | Kind::DurationReq => v
                                .and_then(Value::as_u64)
                                .map(crate::format::format_duration)
                                .unwrap_or_default(),
                            Kind::Size => v
                                .and_then(Value::as_u64)
                                .map(crate::format::format_bytes)
                                .unwrap_or_default(),
                            _ => value_to_text(v),
                        }
                    }
                },
                Target::Admin => value_to_text(persisted.get(key)),
            };
            let entity = cx.new(|cx| TextInput::new(initial.clone(), placeholder, cx));
            rows.push((tab, Row::Input(inputs.len())));
            inputs.push(InputField {
                target,
                key,
                clear_key,
                kind,
                label,
                help,
                input: entity,
                initial,
            });
        };
        let mut boolf = |tab: Tab,
                         key: &'static str,
                         label: &'static str,
                         help: &'static str,
                         rows: &mut Vec<(Tab, Row)>| {
            let v = get_path(&prefs, key)
                .and_then(Value::as_bool)
                .unwrap_or(key.starts_with("download_order.") || key == "confirm_remove");
            rows.push((tab, Row::Bool(bools.len())));
            bools.push(BoolField {
                key,
                label,
                help,
                value: v,
                initial: v,
            });
        };
        let mut trif = |tab: Tab,
                        key: &'static str,
                        clear_key: &'static str,
                        inverted: bool,
                        label: &'static str,
                        help: &'static str,
                        rows: &mut Vec<(Tab, Row)>| {
            let v = tri_from(persisted.get(key), inverted);
            rows.push((tab, Row::Tri(tris.len())));
            tris.push(TriField {
                key,
                clear_key,
                inverted,
                label,
                help,
                value: v,
                initial: v,
            });
        };
        let mut choices: Vec<ChoiceField> = Vec::new();
        let mut choicef = |tab: Tab,
                           key: &'static str,
                           label: &'static str,
                           help: &'static str,
                           options: &'static [(&'static str, &'static str)],
                           default: &'static str,
                           rows: &mut Vec<(Tab, Row)>| {
            let v = choice_value(options, get_path(&prefs, key).and_then(Value::as_str), default);
            rows.push((tab, Row::Choice(choices.len())));
            choices.push(ChoiceField {
                key,
                label,
                help,
                options,
                value: v,
                initial: v,
            });
        };
        let h = |rows: &mut Vec<(Tab, Row)>, tab: Tab, t: &'static str| {
            rows.push((tab, Row::Heading(t)))
        };
        let note = |rows: &mut Vec<(Tab, Row)>, tab: Tab, t: &str| {
            rows.push((tab, Row::Note(t.to_owned().into())))
        };

        use Kind::*;
        use Tab::*;
        use Target as T;

        // Speed
        h(&mut rows, Speed, "Global speed limits");
        note(
            &mut rows,
            Speed,
            "Changes apply immediately and persist to limits.json (overrides CLI/env defaults after save).",
        );
        input(
            Speed,
            T::Limits,
            "download_bps",
            None,
            OptInt,
            "Download rate limit (bytes/s)",
            "Empty or 0 = unlimited. 1048576 = 1 MiB/s.",
            "unlimited",
            &mut rows,
            cx,
        );
        input(
            Speed,
            T::Limits,
            "upload_bps",
            None,
            OptInt,
            "Upload rate limit (bytes/s)",
            "Empty or 0 = unlimited.",
            "unlimited",
            &mut rows,
            cx,
        );

        // Connection
        note(
            &mut rows,
            Connection,
            "These write to admin.json and apply on the next process restart. Environment variables in the systemd unit override the file.",
        );
        h(&mut rows, Connection, "Listening");
        input(
            Connection,
            T::Admin,
            "listen_port",
            Some("clear_listen_port"),
            OptInt,
            "Peer listen port",
            "TCP/uTP listen port. Clear the field and save to remove the override.",
            "startup default",
            &mut rows,
            cx,
        );
        input(
            Connection,
            T::Admin,
            "announce_port",
            Some("clear_announce_port"),
            OptInt,
            "Announce port override",
            "Port advertised to trackers/DHT when behind NAT/forwarding.",
            "startup default",
            &mut rows,
            cx,
        );
        trif(
            Connection,
            "disable_tcp_listen",
            "clear_disable_tcp_listen",
            true,
            "TCP listen",
            "Accept incoming peer connections over TCP.",
            &mut rows,
        );
        trif(
            Connection,
            "enable_utp_listen",
            "clear_enable_utp_listen",
            false,
            "uTP listen (experimental)",
            "Accept incoming peers over uTP/UDP.",
            &mut rows,
        );
        trif(
            Connection,
            "disable_tcp_connect",
            "clear_disable_tcp_connect",
            true,
            "TCP outgoing connects",
            "Dial peers over TCP (disable if using SOCKS/uTP only).",
            &mut rows,
        );
        trif(
            Connection,
            "disable_upnp_port_forward",
            "clear_disable_upnp_port_forward",
            true,
            "UPnP port forwarding",
            "Ask the gateway to forward the listen port.",
            &mut rows,
        );
        h(&mut rows, Connection, "Discovery");
        trif(
            Connection,
            "disable_dht",
            "clear_disable_dht",
            true,
            "DHT",
            "Distributed Hash Table for peer discovery without trackers.",
            &mut rows,
        );
        trif(
            Connection,
            "disable_dht_persistence",
            "clear_disable_dht_persistence",
            true,
            "DHT persistence",
            "Remember DHT routing table across restarts.",
            &mut rows,
        );
        trif(
            Connection,
            "disable_lsd",
            "clear_disable_lsd",
            true,
            "Local service discovery (LSD)",
            "Find peers on the local network via multicast.",
            &mut rows,
        );
        trif(
            Connection,
            "disable_trackers",
            "clear_disable_trackers",
            true,
            "Trackers",
            "Announce to torrent trackers. Private torrents still need trackers.",
            &mut rows,
        );
        h(&mut rows, Connection, "Network / proxy");
        trif(
            Connection,
            "ipv4_only",
            "clear_ipv4_only",
            false,
            "IPv4 only",
            "Bind and connect using IPv4 only.",
            &mut rows,
        );
        input(
            Connection,
            T::Admin,
            "bind_device",
            None,
            Text,
            "Bind device / interface",
            "SO_BINDTODEVICE / IP_BOUND_IF for torrent traffic.",
            "(none)",
            &mut rows,
            cx,
        );
        input(
            Connection,
            T::Admin,
            "socks_proxy_url",
            None,
            Text,
            "SOCKS5 proxy URL",
            "Routes outgoing peer connections through the proxy.",
            "socks5://host:port (empty = none)",
            &mut rows,
            cx,
        );

        // BitTorrent
        h(&mut rows, BitTorrent, "Queueing (live)");
        boolf(
            BitTorrent,
            "queueing_enabled",
            "Enable torrent queueing",
            "Off by default. When on, torrents over the limits below are held as 'Queued' in queue order and start automatically when a slot frees up.",
            &mut rows,
        );
        input(
            BitTorrent,
            T::Prefs,
            "queue_max_active_downloads",
            None,
            OptInt,
            "Maximum active downloads",
            "Torrents downloading at the same time.",
            "no limit",
            &mut rows,
            cx,
        );
        input(
            BitTorrent,
            T::Prefs,
            "queue_max_active_uploads",
            None,
            OptInt,
            "Maximum active uploads (seeding)",
            "Finished torrents seeding at the same time.",
            "no limit",
            &mut rows,
            cx,
        );
        input(
            BitTorrent,
            T::Prefs,
            "queue_max_active_torrents",
            None,
            OptInt,
            "Maximum active torrents",
            "Downloading + seeding in total.",
            "no limit",
            &mut rows,
            cx,
        );
        boolf(
            BitTorrent,
            "queue_ignore_slow_torrents",
            "Don't count slow torrents in these limits",
            "Torrents below 2 KiB/s download and upload for 60 s keep running but don't take a slot.",
            &mut rows,
        );
        input(
            BitTorrent,
            T::Prefs,
            "queue_seed_rotation_secs",
            None,
            Duration,
            "Rotate seeding slots: slot duration",
            "Empty = off (default). Needs 'Maximum active uploads' (= the number of seeding slots). Finished torrents take turns: when a slot's time is up the torrent that has waited longest gets it (fair round robin). A torrent with no leechers yields early when someone is waiting; one with active leechers keeps its slot until another torrent is waiting. Typical: 15m.",
            "off (e.g. 15m)",
            &mut rows,
            cx,
        );
        note(
            &mut rows,
            BitTorrent,
            "How the limits fit together: max downloads / max uploads cap each group, max torrents caps both; rotation only decides which finished torrents use the seeding slots. Automatic rules (Automation tab) still apply — a torrent that hits its seeding limit leaves the rotation.",
        );
        h(&mut rows, BitTorrent, "Peers (live)");
        input(
            BitTorrent,
            T::Prefs,
            "peer_limit",
            None,
            OptInt,
            "Default peers per torrent",
            "Applied immediately to newly added torrents and saved in preferences.json. Does not change already-running torrents' limits.",
            "default",
            &mut rows,
            cx,
        );
        h(
            &mut rows,
            BitTorrent,
            "Peers / init (restart via admin.json)",
        );
        input(
            BitTorrent,
            T::Admin,
            "peer_limit",
            Some("clear_peer_limit"),
            OptInt,
            "Startup peer limit override",
            "Used at the next start.",
            "startup default",
            &mut rows,
            cx,
        );
        input(
            BitTorrent,
            T::Admin,
            "concurrent_init_limit",
            Some("clear_concurrent_init_limit"),
            OptInt,
            "Concurrent torrent initializations",
            "How many torrents can hash/check in parallel at once.",
            "startup default",
            &mut rows,
            cx,
        );
        input(
            BitTorrent,
            T::Admin,
            "peer_connect_timeout_secs",
            Some("clear_peer_connect_timeout_secs"),
            OptInt,
            "Peer connect timeout (seconds)",
            "",
            "startup default",
            &mut rows,
            cx,
        );
        input(
            BitTorrent,
            T::Admin,
            "peer_read_write_timeout_secs",
            Some("clear_peer_read_write_timeout_secs"),
            OptInt,
            "Peer read/write timeout (seconds)",
            "",
            "startup default",
            &mut rows,
            cx,
        );
        h(&mut rows, BitTorrent, "Lists / resume (restart)");
        input(
            BitTorrent,
            T::Admin,
            "blocklist_url",
            None,
            Text,
            "IP blocklist URL",
            "P2P blocklist loaded at startup.",
            "https://… (empty = none)",
            &mut rows,
            cx,
        );
        input(
            BitTorrent,
            T::Admin,
            "allowlist_url",
            None,
            Text,
            "IP allowlist URL",
            "If set, only listed IPs may connect.",
            "https://… (empty = none)",
            &mut rows,
            cx,
        );
        trif(
            BitTorrent,
            "fastresume",
            "clear_fastresume",
            false,
            "Fastresume",
            "Faster session restore after restart by trusting stored piece bitfields.",
            &mut rows,
        );

        // Downloads
        h(&mut rows, Downloads, "Reliability");
        boolf(
            Downloads,
            "soft_recover_on_io_error",
            "Soft-recover on disk I/O errors",
            "When a write fails, invalidate only the affected piece and redownload it instead of fatally stopping the torrent.",
            &mut rows,
        );
        boolf(
            Downloads,
            "auto_repair_damaged_files",
            "Auto-repair damaged files",
            "Needs soft-recover. Automatically punch out unreadable ranges (or copy-and-replace the file) and redownload only the affected pieces.",
            &mut rows,
        );
        input(
            Downloads,
            T::Prefs,
            "recovery_backoff_base_secs",
            None,
            Int,
            "Retry delay after an I/O error (seconds)",
            "Doubles after each consecutive failure (±20% jitter).",
            "60",
            &mut rows,
            cx,
        );
        input(
            Downloads,
            T::Prefs,
            "recovery_backoff_cap_secs",
            None,
            Int,
            "Maximum retry delay (seconds)",
            "Upper bound for the doubling delay (default 21600 = 6 h).",
            "21600",
            &mut rows,
            cx,
        );
        input(
            Downloads,
            T::Prefs,
            "recovery_max_attempts",
            None,
            Int,
            "Give up after N consecutive failures",
            "Then automatic retries stop and the torrent shows 'needs attention'.",
            "8",
            &mut rows,
            cx,
        );
        input(
            Downloads,
            T::Prefs,
            "event_log_max_mb",
            None,
            Int,
            "Event log size limit (MB)",
            "1–1024, default 10.",
            "10",
            &mut rows,
            cx,
        );
        h(&mut rows, Downloads, "Incomplete files");
        input(
            Downloads,
            T::Prefs,
            "incomplete_extension",
            None,
            Text,
            "Incomplete file extension",
            "While downloading, on-disk names get this suffix.",
            "(none)",
            &mut rows,
            cx,
        );
        h(&mut rows, Downloads, "Download order (defaults)");
        note(
            &mut rows,
            Downloads,
            "Order in which pieces are requested. Torrents can override these (right-click a torrent or files → Download order), single files override the torrent. Applied immediately.",
        );
        boolf(
            Downloads,
            "download_order.sequential_files",
            "Sequential file download",
            "Finish files one after another in the file order below. Off: work on all selected files at once.",
            &mut rows,
        );
        choicef(
            Downloads,
            "download_order.file_order",
            "File order",
            "\"By name\" is rqbit's historical behaviour.",
            FILE_ORDERS,
            "name",
            &mut rows,
        );
        boolf(
            Downloads,
            "download_order.sequential",
            "Sequential download (within each file)",
            "Request a file's pieces in order. Off: spread requests across the file.",
            &mut rows,
        );
        boolf(
            Downloads,
            "download_order.first_last_first",
            "Download first and last pieces first",
            "Fetch each file's first and last pieces early (media headers / previews).",
            &mut rows,
        );

        // Automation (rules; all off by default).
        note(
            &mut rows,
            Automation,
            "Automatic rules, all off by default. Global defaults; each torrent can override them in its details (Rules tab). Counters survive restarts and every firing is logged to Events. Rules only delete files when a delete action is chosen.",
        );
        rows.push((Automation, Row::Warnings));
        h(&mut rows, Automation, "Stalled / no progress");
        boolf(
            Automation,
            "rules.stalled.enabled",
            "Act on stalled downloads",
            "Counts only while the torrent is running (not paused, queued or checking). Any newly verified piece resets the timer.",
            &mut rows,
        );
        input(
            Automation,
            T::Prefs,
            "rules.stalled.after_secs",
            None,
            DurationReq,
            "No verified progress for",
            "e.g. 24h, 90m (bare number = minutes).",
            "24h",
            &mut rows,
            cx,
        );
        choicef(
            Automation,
            "rules.stalled.action",
            "Then",
            "\"Remove (finish)\" = finish what's done: unfinished files deleted, completion actions run on finished ones.",
            STALLED_ACTIONS,
            "pause",
            &mut rows,
        );
        h(&mut rows, Automation, "Seeding limits");
        boolf(
            Automation,
            "rules.seeding.enabled",
            "Seeding limits",
            "Whichever limit is reached first fires. Empty = no limit of that kind.",
            &mut rows,
        );
        input(
            Automation,
            T::Prefs,
            "rules.seeding.max_seed_secs",
            None,
            Duration,
            "Seeding time",
            "Counted only while seeding.",
            "e.g. 7d",
            &mut rows,
            cx,
        );
        input(
            Automation,
            T::Prefs,
            "rules.seeding.max_uploaded_bytes",
            None,
            Size,
            "Uploaded",
            "",
            "e.g. 50 GB",
            &mut rows,
            cx,
        );
        input(
            Automation,
            T::Prefs,
            "rules.seeding.max_ratio",
            None,
            Float,
            "Ratio",
            "",
            "e.g. 2.0",
            &mut rows,
            cx,
        );
        choicef(
            Automation,
            "rules.seeding.action",
            "Then",
            "",
            SEEDING_ACTIONS,
            "pause",
            &mut rows,
        );
        h(&mut rows, Automation, "Full-speed window");
        boolf(
            Automation,
            "rules.speed_window.enabled",
            "Full-speed window after completion",
            "Seed without a per-torrent cap for this long (or until this much is uploaded after completion), then cap this torrent's upload or stop seeding. The global upload limit still applies.",
            &mut rows,
        );
        input(
            Automation,
            T::Prefs,
            "rules.speed_window.full_speed_secs",
            None,
            Duration,
            "Full speed for",
            "",
            "e.g. 2h",
            &mut rows,
            cx,
        );
        input(
            Automation,
            T::Prefs,
            "rules.speed_window.full_speed_bytes",
            None,
            Size,
            "or until uploaded",
            "",
            "e.g. 10 GB",
            &mut rows,
            cx,
        );
        choicef(
            Automation,
            "rules.speed_window.then",
            "Then",
            "",
            WINDOW_THEN,
            "cap",
            &mut rows,
        );
        input(
            Automation,
            T::Prefs,
            "rules.speed_window.cap_kib_per_sec",
            None,
            Int,
            "Upload cap (KB/s)",
            "Used when \"Cap upload rate\" is chosen.",
            "100",
            &mut rows,
            cx,
        );

        // Organize
        h(&mut rows, Organize, "Auto-organize");
        boolf(
            Organize,
            "auto_organize_enabled",
            "Auto-organize completed torrents",
            "When enabled (and no custom action list), finished torrents are classified and moved under the organize root. Heuristics can be wrong.",
            &mut rows,
        );
        input(
            Organize,
            T::Prefs,
            "auto_organize_root",
            None,
            Text,
            "Auto-organize root",
            "Destination becomes root/<type>/<torrent-folder>.",
            "(empty = session download folder)",
            &mut rows,
            cx,
        );
        h(&mut rows, Organize, "Type → subfolder names");
        for (k, d) in ORGANIZE_FOLDERS {
            let key: &'static str =
                Box::leak(format!("auto_organize_folders.{k}").into_boxed_str());
            input(Organize, T::Prefs, key, None, Text, k, "", d, &mut rows, cx);
        }

        // Completion
        h(&mut rows, Completion, "Action pipeline");
        let actions = prefs
            .get("completion_actions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if actions.is_empty() {
            note(
                &mut rows,
                Completion,
                "No custom actions: the legacy options below apply.",
            );
        } else {
            for (i, a) in actions.iter().enumerate() {
                let ty = a.get("type").and_then(Value::as_str).unwrap_or("?");
                let desc = match ty {
                    "move" => format!(
                        "Move completed → {}{}",
                        a.get("path").and_then(Value::as_str).unwrap_or(""),
                        if a.get("copy").and_then(Value::as_bool) == Some(true) {
                            " (copy)"
                        } else {
                            ""
                        }
                    ),
                    "organize" => "Auto-organize".into(),
                    "drop_incomplete_ext" => "Drop incomplete extension".into(),
                    "shell" => format!(
                        "Shell: {}",
                        a.get("command").and_then(Value::as_str).unwrap_or("")
                    ),
                    other => other.to_owned(),
                };
                note(&mut rows, Completion, &format!("{}. {desc}", i + 1));
            }
        }
        note(
            &mut rows,
            Completion,
            "Editing the action pipeline is only available in the web UI (Configure → Completion) for now; it is preserved when saving here.",
        );
        h(&mut rows, Completion, "Legacy options");
        input(
            Completion,
            T::Prefs,
            "on_complete_hook",
            None,
            Text,
            "On-complete hook (shell)",
            "Env: RQBIT_TORRENT_ID, RQBIT_INFO_HASH, RQBIT_NAME, RQBIT_OUTPUT_FOLDER.",
            "(none)",
            &mut rows,
            cx,
        );
        input(
            Completion,
            T::Prefs,
            "move_completed_path",
            None,
            Text,
            "Move completed to",
            "Move (or copy) torrent files here when the download finishes.",
            "(none)",
            &mut rows,
            cx,
        );
        boolf(
            Completion,
            "move_completed_copy",
            "Copy instead of move when completing",
            "Leave originals in place and copy into the completed folder.",
            &mut rows,
        );

        // Admin (read-only)
        h(&mut rows, Admin, "Runtime status");
        let info = |rows: &mut Vec<(Tab, Row)>, l: &'static str, v: String| {
            rows.push((Admin, Row::Info(l, v)))
        };
        info(&mut rows, "version", admin.version.clone());
        info(&mut rows, "preferences", admin.preferences_path.clone());
        info(&mut rows, "admin.json", admin.admin_path.clone());
        info(
            &mut rows,
            "effective listen",
            admin
                .effective_http_listen_addr
                .clone()
                .unwrap_or_else(|| "?".into()),
        );
        info(
            &mut rows,
            "env listen",
            admin
                .env_http_listen_addr
                .clone()
                .unwrap_or_else(|| "(unset)".into()),
        );
        info(
            &mut rows,
            "env basic auth",
            if admin.env_basic_auth_set {
                "set".into()
            } else {
                "unset".into()
            },
        );
        info(
            &mut rows,
            "restart API",
            if admin.restart_supported {
                "available".into()
            } else {
                "not available".into()
            },
        );
        h(&mut rows, Admin, "HTTP API (restart required)");
        input(
            Admin,
            Target::Admin,
            "http_api_listen_addr",
            None,
            Kind::Text,
            "Listen address",
            "Host:port for the HTTP API / web UI. Applied on next start if RQBIT_HTTP_API_LISTEN_ADDR is unset.",
            "0.0.0.0:9030",
            &mut rows,
            cx,
        );
        rows.push((Admin, Row::Auth));
        for n in &admin.notes {
            note(&mut rows, Admin, n);
        }
        h(&mut rows, Admin, "Operations");
        rows.push((Admin, Row::AdminOps));
        let initial_enabled = persisted
            .get("basic_auth_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let initial_user = value_to_text(persisted.get("basic_auth_user"));
        let password_set = persisted
            .get("basic_auth_password_set")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let auth = AuthFields {
            enabled: initial_enabled,
            initial_enabled,
            user: cx.new(|cx| TextInput::new(initial_user.clone(), "username", cx)),
            initial_user,
            password: cx.new(|cx| {
                TextInput::new(
                    "",
                    if password_set {
                        "leave blank to keep current"
                    } else {
                        "password"
                    },
                    cx,
                )
                .masked()
            }),
            password_set,
        };

        // Interface (server-persisted UI preferences shared with the web UI).
        h(&mut rows, Interface, "Adding torrents");
        choicef(
            Interface,
            "when_added",
            "When a torrent is added",
            "Default for every add (GPUI, web UI, API, watch folder) that doesn't say itself. Magnets still fetch their metadata while paused.",
            WHEN_ADDED,
            "start",
            &mut rows,
        );
        boolf(
            Interface,
            "start_after_add_dialog",
            "Start after I finish the Add dialog",
            "Torrents added from an Add panel stay paused while it is open, so you can adjust files, priorities or the folder, and all start when you close it. Not with \u{201c}Add paused\u{201d}; API and watch-folder adds are unaffected.",
            &mut rows,
        );
        h(&mut rows, Interface, "Removing torrents");
        boolf(
            Interface,
            "confirm_remove",
            "Confirm before removing torrents",
            "Show the remove dialog (toolbar, right-click menu, Delete key). Anything that deletes files always shows it. Also used by the web UI.",
            &mut rows,
        );
        choicef(
            Interface,
            "remove_policy.complete",
            "When the torrent is complete",
            "",
            COMPLETE_ACTIONS,
            "keep",
            &mut rows,
        );
        choicef(
            Interface,
            "remove_policy.incomplete",
            "When the torrent is incomplete",
            "Finish what's done: unfinished files are deselected and their partial data deleted (only files this torrent created), completion actions run on the finished files, then the torrent is removed keeping those files. If an action fails the torrent is kept and flagged. With nothing finished, all partial data is deleted.",
            INCOMPLETE_ACTIONS,
            "keep",
            &mut rows,
        );
        rows.push((Interface, Row::Warnings));
        h(&mut rows, Interface, "Magnet links and .torrent files");
        rows.push((Interface, Row::Handlers));

        self.loaded = Some(Loaded {
            limits,
            prefs,
            inputs,
            bools,
            tris,
            choices,
            rows,
            auth,
            restart_supported: admin.restart_supported,
        });
    }

    #[cfg(not(target_family = "wasm"))]
    fn refresh_handler_status(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let s = cx
                .background_executor()
                .spawn(async move {
                    use crate::handlers::register as reg;
                    reg::Launcher::current()
                        .map(|l| reg::status(&l).describe())
                        .unwrap_or_else(|e| format!("Can't find the rqbit executable: {e}"))
                })
                .await;
            this.update(cx, |p, cx| {
                p.handler_status = Some(s);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    #[cfg(target_family = "wasm")]
    fn refresh_handler_status(&mut self, _cx: &mut Context<Self>) {
        self.handler_status = Some(
            "Your browser can open magnet links with this page: they appear in the Add window, ready to add.".into(),
        );
    }

    #[cfg(not(target_family = "wasm"))]
    fn set_handlers(&mut self, register: bool, cx: &mut Context<Self>) {
        self.handler_msg = Some(if register { "Registering…" } else { "Removing…" }.into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let msg = cx
                .background_executor()
                .spawn(async move {
                    use crate::handlers::register as reg;
                    let l = match reg::Launcher::current() {
                        Ok(l) => l,
                        Err(e) => return format!("Can't find the rqbit executable: {e}"),
                    };
                    if !register {
                        reg::unregister(&l);
                        return "rqbit is no longer registered.".into();
                    }
                    match reg::register(&l) {
                        Err(e) => format!("Couldn't register: {e}"),
                        Ok(notes) => {
                            let mut m = notes.join(" ");
                            if cfg!(windows) && !reg::status(&l).is_default() {
                                reg::open_default_apps_settings();
                                m.push_str(" Windows doesn't let apps make themselves the default: in the Settings page that opened, choose rqbit for MAGNET and .torrent.");
                            }
                            m.trim().to_owned()
                        }
                    }
                })
                .await;
            this.update(cx, |p, cx| {
                p.handler_msg = Some(msg);
                p.refresh_handler_status(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    #[cfg(target_family = "wasm")]
    fn set_handlers(&mut self, _register: bool, cx: &mut Context<Self>) {
        self.handler_msg = Some(match crate::launch::register_browser_magnet_handler() {
            Ok(_) => "Asked the browser to open magnet links here; confirm in its prompt (if it shows one).".into(),
            Err(e) => e,
        });
        cx.notify();
    }

    fn render_handlers(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let native = cfg!(not(target_family = "wasm"));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(self.handler_status.clone().unwrap_or_else(|| "Checking…".into())),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        widgets::button(
                            "handlers-register",
                            if native {
                                "Make rqbit the default for magnet links and .torrent files"
                            } else {
                                "Register as magnet handler in this browser"
                            },
                            true,
                        )
                        .on_click(cx.listener(|p, _, _, cx| p.set_handlers(true, cx))),
                    )
                    .when(native, |d| {
                        d.child(
                            widgets::button("handlers-unregister", "Unregister", true)
                                .on_click(cx.listener(|p, _, _, cx| p.set_handlers(false, cx))),
                        )
                    }),
            )
            .children(
                self.handler_msg
                    .clone()
                    .map(|m| div().text_xs().text_color(theme::text()).child(m)),
            )
            .child(div().text_xs().text_color(theme::text_muted()).child(if native {
                "Applies to this computer (your user only). Command line: rqbit-gpui --register-handlers / --unregister-handlers."
            } else {
                "Applies to this browser. Browsers ask you to confirm and may only allow it on https pages."
            }))
            .into_any_element()
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(l) = &self.loaded else { return };
        let inputs: Vec<FieldValue> = l
            .inputs
            .iter()
            .map(|f| FieldValue {
                target: f.target,
                key: f.key,
                clear_key: f.clear_key,
                kind: f.kind,
                label: f.label,
                text: f.input.read(cx).text().to_owned(),
                initial: f.initial.clone(),
            })
            .collect();
        let bools: Vec<(&str, bool, bool)> = l
            .bools
            .iter()
            .map(|b| (b.key, b.value, b.initial))
            .collect();
        let tris: Vec<(&str, &str, bool, Tri, Tri)> = l
            .tris
            .iter()
            .map(|t| (t.key, t.clear_key, t.inverted, t.value, t.initial))
            .collect();
        let auth = auth_patch(
            l.auth.enabled,
            l.auth.initial_enabled,
            l.auth.user.read(cx).text(),
            &l.auth.initial_user,
            l.auth.password.read(cx).text(),
        );
        let choices: Vec<(&str, &str, &str)> = l
            .choices
            .iter()
            .map(|c| (c.key, c.value, c.initial))
            .collect();
        let plan = match plan_save(&l.limits, &l.prefs, &inputs, &bools, &tris) {
            Ok(mut p) => {
                apply_choices(&mut p, &l.prefs, &choices);
                if !auth.is_empty() {
                    p.admin.get_or_insert_with(Map::new).extend(auth);
                }
                p
            }
            Err(errs) => {
                self.error = Some(errs.join("\n"));
                self.message = None;
                cx.notify();
                return;
            }
        };
        if plan == SavePlan::default() {
            self.message = Some("Nothing changed.".into());
            self.error = None;
            cx.notify();
            return;
        }
        self.saving = true;
        self.error = None;
        self.message = None;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let bg = cx.background_executor().clone();
            let mut done = Vec::new();
            let mut res: anyhow::Result<()> = Ok(());
            if let Some(l) = &plan.limits {
                res = bg.spawn(client.set_limits(l)).await;
                if res.is_ok() {
                    done.push("speed limits");
                }
            }
            if res.is_ok()
                && let Some(p) = &plan.prefs
            {
                res = bg.spawn(client.set_preferences(p)).await;
                if res.is_ok() {
                    done.push("preferences");
                }
            }
            if res.is_ok()
                && let Some(a) = plan.admin
            {
                res = bg
                    .spawn(client.update_admin_config(&Value::Object(a)))
                    .await;
                if res.is_ok() {
                    done.push("admin.json (applies on restart)");
                }
            }
            this.update(cx, |p, cx| {
                p.saving = false;
                match res {
                    Ok(()) => {
                        p.message = Some(format!("Saved {}.", done.join(", ")));
                        p.reload(cx);
                    }
                    Err(e) => {
                        p.error = Some(format!(
                            "Error saving configuration{}: {e:#}",
                            if done.is_empty() {
                                String::new()
                            } else {
                                format!(" (saved: {})", done.join(", "))
                            }
                        ))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// `POST /admin/reload` or `/admin/restart`.
    fn admin_op(&mut self, restart: bool, cx: &mut Context<Self>) {
        self.confirm_restart = false;
        self.saving = true;
        self.error = None;
        self.message = None;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let fut = if restart {
                client.admin_restart()
            } else {
                client.admin_reload()
            };
            let r = cx.background_executor().spawn(fut).await;
            this.update(cx, |p, cx| {
                p.saving = false;
                match r {
                    Ok(()) if restart => {
                        p.message = Some(
                            "Restart signaled. The client reconnects once the service is back."
                                .into(),
                        )
                    }
                    Ok(()) => {
                        p.message = Some(
                            "Reloaded preferences.json from disk into the live session.".into(),
                        );
                        p.reload(cx);
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

    fn render_rows(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let Some(l) = &self.loaded else {
            return Vec::new();
        };
        let tab = self.tab;
        l.rows
            .iter()
            .filter(|(t, _)| *t == tab)
            .enumerate()
            .map(|(n, (_, row))| match row {
                Row::Heading(t) => div()
                    .pt_2()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(*t)
                    .into_any_element(),
                Row::Note(t) => div()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(t.clone())
                    .into_any_element(),
                Row::Warnings => {
                    let w = delete_warnings(l);
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(w.into_iter().map(|t| {
                            div()
                                .text_xs()
                                .p_1()
                                .rounded_md()
                                .border_1()
                                .border_color(theme::error())
                                .text_color(theme::error())
                                .child(t)
                        }))
                        .into_any_element()
                }
                Row::Info(label, v) => div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .w(px(130.))
                            .flex_shrink_0()
                            .text_color(theme::text_muted())
                            .child(*label),
                    )
                    .child(div().text_color(theme::text()).child(v.clone()))
                    .into_any_element(),
                Row::Input(i) => {
                    let f = &l.inputs[*i];
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().child(f.label))
                        .child(div().max_w(px(460.)).child(f.input.clone()))
                        .when(!f.help.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(f.help),
                            )
                        })
                        .into_any_element()
                }
                Row::Auth => {
                    let a = &l.auth;
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            widgets::checkbox(
                                "pref-auth",
                                "Enable HTTP basic authentication",
                                a.enabled,
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(l) = &mut this.loaded {
                                    l.auth.enabled = !l.auth.enabled;
                                }
                                cx.notify();
                            })),
                        )
                        .child(
                            div()
                                .pl(px(24.))
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child("Username/password for the API and web UI. Applied on next start if RQBIT_HTTP_BASIC_AUTH_USERPASS is unset."),
                        )
                        .when(a.enabled, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .pl(px(24.))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .w(px(220.))
                                            .child(div().text_sm().child("Username"))
                                            .child(a.user.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .w(px(220.))
                                            .child(div().text_sm().child(if a.password_set {
                                                "Password (blank = keep)"
                                            } else {
                                                "Password"
                                            }))
                                            .child(a.password.clone()),
                                    ),
                            )
                        })
                        .into_any_element()
                }
                Row::Handlers => self.render_handlers(cx),
                Row::AdminOps => {
                    let enabled = !self.saving;
                    let restart_ok = enabled && l.restart_supported;
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(
                                    widgets::button(
                                        "admin-reload",
                                        "Force-reload preferences.json",
                                        enabled,
                                    )
                                    .when(enabled, |b| {
                                        b.on_click(
                                            cx.listener(|this, _, _, cx| this.admin_op(false, cx)),
                                        )
                                    }),
                                )
                                .when(!self.confirm_restart, |d| {
                                    d.child(
                                        widgets::button(
                                            "admin-restart",
                                            "Restart rqbit…",
                                            restart_ok,
                                        )
                                        .when(restart_ok, |b| {
                                            b.on_click(cx.listener(|this, _, _, cx| {
                                                this.confirm_restart = true;
                                                cx.notify();
                                            }))
                                        }),
                                    )
                                }),
                        )
                        .when(self.confirm_restart, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(div().text_xs().child(
                                        "Restart the rqbit process? Torrents resume after the service manager brings it back.",
                                    ))
                                    .child(
                                        widgets::danger_button("admin-restart-yes", "Restart")
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.admin_op(true, cx)),
                                            ),
                                    )
                                    .child(
                                        widgets::button("admin-restart-no", "Cancel", true)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.confirm_restart = false;
                                                cx.notify();
                                            })),
                                    ),
                            )
                        })
                        .when(!l.restart_supported, |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child("Restart API not available on this server."),
                            )
                        })
                        .into_any_element()
                }
                Row::Bool(i) => {
                    let f = &l.bools[*i];
                    let i = *i;
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            widgets::checkbox(("pref-bool", n), f.label, f.value, true).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(l) = &mut this.loaded {
                                        l.bools[i].value = !l.bools[i].value;
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            div()
                                .pl(px(24.))
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(f.help),
                        )
                        .into_any_element()
                }
                Row::Choice(i) => {
                    let f = &l.choices[*i];
                    let i = *i;
                    let last = f.options.len().saturating_sub(1);
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().child(f.label))
                        .child(div().flex().flex_row().children(
                            f.options.iter().enumerate().map(|(k, (value, label))| {
                                let value: &'static str = value;
                                widgets::segment(
                                    ("pref-choice", n * 8 + k),
                                    *label,
                                    f.value == value,
                                )
                                .when(k == 0, |d| d.rounded_l_md())
                                .when(k == last, |d| d.rounded_r_md())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(l) = &mut this.loaded {
                                        l.choices[i].value = value;
                                    }
                                    cx.notify();
                                }))
                            }),
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .when(f.help.is_empty(), |d| d.hidden())
                                .child(f.help),
                        )
                        .into_any_element()
                }
                Row::Tri(i) => {
                    let f = &l.tris[*i];
                    let i = *i;
                    let seg = |id: usize, label: &'static str, v: Tri| {
                        widgets::segment(("pref-tri", n * 3 + id), label, f.value == v).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(l) = &mut this.loaded {
                                    l.tris[i].value = v;
                                }
                                cx.notify();
                            }),
                        )
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().child(f.label))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .child(
                                    seg(0, "Startup default (CLI/env)", Tri::Default)
                                        .rounded_l_md(),
                                )
                                .child(seg(1, "Enabled", Tri::On))
                                .child(seg(2, "Disabled", Tri::Off).rounded_r_md()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(f.help),
                        )
                        .into_any_element()
                }
            })
            .collect()
    }
}

impl Render for PrefsPanel {
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
                    .child("Preferences"),
            )
            .child(div().flex_1())
            .child(
                widgets::button("prefs-close-x", "×", true)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PrefsPanelEvent::Close))),
            );

        let tabs = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_1()
            .px_4()
            .pt_2()
            .children(TABS.iter().enumerate().map(|(i, (t, label))| {
                let t = *t;
                let active = self.tab == t;
                div()
                    .id(("prefs-tab", i))
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = t;
                        cx.notify();
                    }))
                    .child(*label)
            }));

        let body = div()
            .id("prefs-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .gap_3()
            .p_4()
            .when(self.loading && self.loaded.is_none(), |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(theme::text_muted())
                        .child("Loading…"),
                )
            })
            .children(self.render_rows(cx));

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .when_some(self.error.clone(), |d, e| {
                        d.text_color(theme::error()).child(e)
                    })
                    .when(self.error.is_none(), |d| {
                        d.text_color(theme::success())
                            .children(self.message.clone())
                    }),
            )
            .child(
                widgets::button("prefs-reload", "Reload", !self.saving).when(!self.saving, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| {
                        this.message = None;
                        this.error = None;
                        this.reload(cx);
                        cx.notify();
                    }))
                }),
            )
            .child(
                widgets::button("prefs-close", "Close", true)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PrefsPanelEvent::Close))),
            )
            .child(
                widgets::primary_button(
                    "prefs-save",
                    if self.saving { "Saving…" } else { "Save" },
                    self.loaded.is_some() && !self.saving,
                )
                .when(self.loaded.is_some() && !self.saving, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                }),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(tabs)
            .child(body)
            .child(footer)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn choices_join_the_prefs_save() {
        let prefs: Map<String, Value> =
            serde_json::from_value(json!({"peer_limit": 5, "future_field": 1})).unwrap();
        let mut plan = SavePlan::default();
        apply_choices(
            &mut plan,
            &prefs,
            &[("remove_policy.incomplete", "keep", "keep")],
        );
        assert_eq!(plan, SavePlan::default());
        apply_choices(
            &mut plan,
            &prefs,
            &[("remove_policy.incomplete", "finish", "keep")],
        );
        let p = plan.prefs.expect("prefs saved");
        assert_eq!(p["remove_policy"]["incomplete"], "finish");
        assert_eq!(p["future_field"], 1);
        assert_eq!(p["peer_limit"], 5);
    }

    use super::*;

    #[test]
    fn nested_paths_and_unit_kinds() {
        let prefs: Map<String, Value> = serde_json::from_value(json!({
            "rules": {"stalled": {"enabled": false, "after_secs": 86400, "action": "pause"}},
            "other": 1
        }))
        .unwrap();
        assert_eq!(get_path(&prefs, "rules.stalled.after_secs"), Some(&json!(86400)));
        assert_eq!(get_path(&prefs, "rules.seeding.enabled"), None);
        let inputs = [
            fv(Target::Prefs, "rules.stalled.after_secs", None, Kind::DurationReq, "2h 30m", "1d"),
            fv(Target::Prefs, "rules.seeding.max_uploaded_bytes", None, Kind::Size, "1.5 GB", ""),
            fv(Target::Prefs, "rules.seeding.max_ratio", None, Kind::Float, "2", ""),
            fv(Target::Prefs, "queue_seed_rotation_secs", None, Kind::Duration, "", "15m"),
        ];
        let plan = plan_save(&LimitsConfig::default(), &prefs, &inputs, &[("rules.seeding.enabled", true, false)], &[])
            .expect("valid");
        let p = plan.prefs.expect("changed");
        assert_eq!(p["rules"]["stalled"]["after_secs"], 9000);
        assert_eq!(p["rules"]["stalled"]["action"], "pause", "siblings kept");
        assert_eq!(p["rules"]["seeding"]["max_uploaded_bytes"], 1610612736u64);
        assert_eq!(p["rules"]["seeding"]["max_ratio"], 2.0);
        assert_eq!(p["rules"]["seeding"]["enabled"], true);
        assert_eq!(p["queue_seed_rotation_secs"], Value::Null, "empty = rotation off");
        assert_eq!(p["other"], 1);
        let bad = [fv(Target::Prefs, "rules.stalled.after_secs", None, Kind::DurationReq, "soon", "1d")];
        assert!(plan_save(&LimitsConfig::default(), &prefs, &bad, &[], &[]).is_err());
    }

    #[test]
    fn delete_warning_rules() {
        assert!(warnings_for(("keep", "keep"), false, None, None).is_empty());
        assert_eq!(warnings_for(("keep", "finish"), false, None, None).len(), 1);
        assert!(warnings_for(("keep", "finish"), true, None, None).is_empty());
        assert_eq!(warnings_for(("keep", "keep"), true, Some("remove_delete"), None).len(), 1);
        assert!(warnings_for(("keep", "keep"), true, Some("pause"), Some("remove_policy")).is_empty());
        assert_eq!(warnings_for(("delete", "keep"), true, None, Some("remove_policy")).len(), 1);
    }

    #[test]
    fn auth_patches() {
        assert!(auth_patch(false, false, "", "", "").is_empty());
        assert!(auth_patch(true, true, "bob", "bob", "").is_empty());
        let m = auth_patch(false, true, "bob", "bob", "");
        assert_eq!(Value::Object(m), json!({"basic_auth_enabled": false}));
        let m = auth_patch(true, true, "bob", "bob", "pw");
        assert_eq!(
            Value::Object(m),
            json!({"basic_auth_enabled": true, "basic_auth_user": "bob", "basic_auth_password": "pw"})
        );
        let m = auth_patch(true, false, " al ", "", "");
        assert_eq!(
            Value::Object(m),
            json!({"basic_auth_enabled": true, "basic_auth_user": "al"})
        );
    }

    fn fv<'a>(
        target: Target,
        key: &'a str,
        clear: Option<&'a str>,
        kind: Kind,
        text: &str,
        initial: &str,
    ) -> FieldValue<'a> {
        FieldValue {
            target,
            key,
            clear_key: clear,
            kind,
            label: key,
            text: text.into(),
            initial: initial.into(),
        }
    }

    #[test]
    fn unchanged_sends_nothing() {
        let prefs = Map::new();
        let plan = plan_save(
            &LimitsConfig::default(),
            &prefs,
            &[fv(
                Target::Prefs,
                "peer_limit",
                None,
                Kind::OptInt,
                "5",
                "5",
            )],
            &[("queueing_enabled", false, false)],
            &[(
                "disable_dht",
                "clear_disable_dht",
                true,
                Tri::Default,
                Tri::Default,
            )],
        )
        .unwrap();
        assert_eq!(plan, SavePlan::default());
    }

    #[test]
    fn changes_are_mapped_like_the_web_ui() {
        let mut prefs = Map::new();
        prefs.insert("unknown_future".into(), json!(1));
        prefs.insert("on_complete_hook".into(), json!("x"));
        let plan = plan_save(
            &LimitsConfig::default(),
            &prefs,
            &[
                fv(
                    Target::Limits,
                    "download_bps",
                    None,
                    Kind::OptInt,
                    "1048576",
                    "",
                ),
                fv(
                    Target::Prefs,
                    "on_complete_hook",
                    None,
                    Kind::Text,
                    "  ",
                    "x",
                ),
                fv(
                    Target::Prefs,
                    "queue_max_active_downloads",
                    None,
                    Kind::OptInt,
                    "3",
                    "",
                ),
                fv(
                    Target::Prefs,
                    "auto_organize_folders.tv",
                    None,
                    Kind::Text,
                    "Shows",
                    "TV",
                ),
                fv(
                    Target::Admin,
                    "listen_port",
                    Some("clear_listen_port"),
                    Kind::OptInt,
                    "",
                    "4240",
                ),
                fv(
                    Target::Admin,
                    "peer_limit",
                    Some("clear_peer_limit"),
                    Kind::OptInt,
                    "200",
                    "",
                ),
                fv(
                    Target::Admin,
                    "socks_proxy_url",
                    None,
                    Kind::Text,
                    "",
                    "socks5://x",
                ),
            ],
            &[("queueing_enabled", true, false)],
            &[
                (
                    "disable_dht",
                    "clear_disable_dht",
                    true,
                    Tri::Off,
                    Tri::Default,
                ),
                (
                    "fastresume",
                    "clear_fastresume",
                    false,
                    Tri::Default,
                    Tri::On,
                ),
            ],
        )
        .unwrap();
        assert_eq!(plan.limits.unwrap().download_bps, Some(1048576));
        let p = plan.prefs.unwrap();
        assert_eq!(p["unknown_future"], json!(1));
        assert_eq!(p["on_complete_hook"], Value::Null);
        assert_eq!(p["queue_max_active_downloads"], json!(3));
        assert_eq!(p["queueing_enabled"], json!(true));
        assert_eq!(p["auto_organize_folders"]["tv"], json!("Shows"));
        assert_eq!(p["auto_organize_folders"]["movie"], json!("Movies"));
        let a = plan.admin.unwrap();
        assert_eq!(a["clear_listen_port"], json!(true));
        assert_eq!(a["peer_limit"], json!(200));
        assert_eq!(a["socks_proxy_url"], json!(""));
        // DHT "Disabled" -> disable_dht = true
        assert_eq!(a["disable_dht"], json!(true));
        assert_eq!(a["clear_fastresume"], json!(true));
    }

    #[test]
    fn invalid_numbers_are_reported() {
        let e = plan_save(
            &LimitsConfig::default(),
            &Map::new(),
            &[fv(
                Target::Prefs,
                "recovery_max_attempts",
                None,
                Kind::Int,
                "abc",
                "8",
            )],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(e[0].contains("not a whole number"));
        let e = plan_save(
            &LimitsConfig::default(),
            &Map::new(),
            &[fv(
                Target::Limits,
                "upload_bps",
                None,
                Kind::OptInt,
                "99999999999",
                "",
            )],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(e[0].contains("too large"));
    }

    #[test]
    fn tri_mapping() {
        assert_eq!(tri_from(None, true), Tri::Default);
        assert_eq!(tri_from(Some(&json!(true)), true), Tri::Off);
        assert_eq!(tri_from(Some(&json!(false)), true), Tri::On);
        assert_eq!(tri_from(Some(&json!(true)), false), Tri::On);
    }
}
