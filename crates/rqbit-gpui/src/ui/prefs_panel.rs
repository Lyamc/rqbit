//! Preferences, backed by the same endpoints as the web UI's Configure
//! dialog (`ConfigModal.tsx`): `GET/POST /torrents/limits`,
//! `GET/POST /torrents/preferences`, `GET /admin` + `POST /admin/config`.
//! Same tabs: Speed, Connection, BitTorrent, Downloads, Organize,
//! Completion, Web UI / Admin.
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
    Admin,
}

const TABS: [(Tab, &str); 7] = [
    (Tab::Speed, "Speed"),
    (Tab::Connection, "Connection"),
    (Tab::BitTorrent, "BitTorrent"),
    (Tab::Downloads, "Downloads"),
    (Tab::Organize, "Organize"),
    (Tab::Completion, "Completion"),
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
    Info(&'static str, String),
}

struct Loaded {
    limits: LimitsConfig,
    prefs: Map<String, Value>,
    inputs: Vec<InputField>,
    bools: Vec<BoolField>,
    tris: Vec<TriField>,
    rows: Vec<(Tab, Row)>,
}

pub struct PrefsPanel {
    client: ApiClient,
    tab: Tab,
    loading: bool,
    saving: bool,
    error: Option<String>,
    message: Option<String>,
    loaded: Option<Loaded>,
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
                    new_prefs.insert(f.key.to_owned(), value);
                }
                prefs_changed = true;
            }
            Target::Admin => match f.kind {
                Kind::Text => {
                    admin.insert(f.key.to_owned(), json!(text));
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
            new_prefs.insert((*key).to_owned(), json!(value));
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
        };
        this.reload(cx);
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
                    None => value_to_text(prefs.get(key)),
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
            let v = prefs.get(key).and_then(Value::as_bool).unwrap_or(false);
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
        info(
            &mut rows,
            "listen address",
            value_to_text(persisted.get("http_api_listen_addr")),
        );
        info(
            &mut rows,
            "basic auth",
            match persisted.get("basic_auth_enabled").and_then(Value::as_bool) {
                Some(true) => format!(
                    "enabled (user {})",
                    value_to_text(persisted.get("basic_auth_user"))
                ),
                _ => "disabled".into(),
            },
        );
        for n in &admin.notes {
            note(&mut rows, Admin, n);
        }
        note(
            &mut rows,
            Admin,
            "Editing the HTTP listen address / basic auth and Reload / Restart are in the web UI (Configure → Web UI / Admin) for now.",
        );

        self.loaded = Some(Loaded {
            limits,
            prefs,
            inputs,
            bools,
            tris,
            rows,
        });
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
        let plan = match plan_save(&l.limits, &l.prefs, &inputs, &bools, &tris) {
            Ok(p) => p,
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
    use super::*;

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
