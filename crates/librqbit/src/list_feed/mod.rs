//! Lean torrent-list feed for the UIs: the rows of `GET /torrents?with_stats=true`
//! without the fields no UI reads, sent as one snapshot and then deltas.
//!
//! Used by `GET /stream/torrents` (WebSocket) and its `?since=` polling fallback
//! (see `http_api/handlers/list_stream.rs`). `/torrents?with_stats=true` itself is
//! unchanged.
//!
//! # Row format ("lean")
//! A row is the list item as JSON with these changes (lossless for every field a UI
//! reads):
//! - nulls are left out (absent = null);
//! - `stats.live.download_speed` / `upload_speed` (`{mbps, human_readable}`) become
//!   `download_bps` / `upload_bps`: integer bytes/s, exactly `mbps * 2^20`;
//! - `stats.live.time_remaining` (`{duration, human_readable}`) becomes `eta_ms`;
//! - dropped: `stats.live.average_piece_download_time`,
//!   `stats.live.snapshot.{downloaded_and_checked_bytes,total_piece_download_ms}`,
//!   `stats.live.snapshot.peer_stats.{live_tcp,live_utp,live_socks,live_seeders,steals}`.
//!
//! Clients format the human-readable strings themselves (same output as the server's
//! `Speed` / `DurationWithHumanReadable`).
//!
//! # Messages (JSON; `proto` = [`PROTO`])
//! - `{"type":"snapshot","proto":1,"epoch":E,"seq":N,"torrents":[row,..]}`
//! - `{"type":"delta","proto":1,"epoch":E,"base":B,"seq":N,"added":[row,..],
//!   "removed":[id,..],"changed":[patch,..],"order":[id,..]}`: turns the state at `B`
//!   into the state at `N`. Empty lists are left out. Each `changed` entry is an RFC 7386
//!   JSON merge patch of one row plus its `"id"`: only the changed fields, nested objects
//!   patched recursively, arrays replaced whole, `null` = field removed. Static fields
//!   (name, folder, category, ..) and dynamic ones (stats) go through the same patches,
//!   so static ones are only sent when they change. Rows keep the server's order:
//!   removed ones drop out, added ones are appended, and `order` (all ids) is sent only
//!   when that rule doesn't give the server's order.
//! - `{"type":"heartbeat","epoch":E,"seq":N}` (WebSocket, when nothing else was sent
//!   for a while), `{"type":"pong"}`.
//!
//! A client applies a delta only if `epoch` matches and `base` equals its `seq`;
//! otherwise it asks for a resync (WebSocket `{"type":"resync"}`, or polls without
//! `since`) and gets a snapshot.

use serde_json::{Map, Value};

/// Protocol version in every message.
pub const PROTO: u32 = 1;

const DROP_LIVE: &[&str] = &["average_piece_download_time"];
const DROP_SNAPSHOT: &[&str] = &["downloaded_and_checked_bytes", "total_piece_download_ms"];
const DROP_PEER_STATS: &[&str] = &[
    "live_tcp",
    "live_utp",
    "live_socks",
    "live_seeders",
    "steals",
];

/// Removes object members that are null, recursively (array elements are kept).
pub fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, v| !v.is_null());
            m.values_mut().for_each(strip_nulls);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// One `/torrents?with_stats=true` item (as JSON) → lean row.
pub fn lean_row(mut v: Value) -> Value {
    strip_nulls(&mut v);
    if let Some(o) = v.as_object_mut() {
        o.remove("files");
    }
    if let Some(live) = v.pointer_mut("/stats/live").and_then(Value::as_object_mut) {
        for k in DROP_LIVE {
            live.remove(*k);
        }
        for (from, to) in [
            ("download_speed", "download_bps"),
            ("upload_speed", "upload_bps"),
        ] {
            if let Some(s) = live.remove(from) {
                let mbps = s.get("mbps").and_then(Value::as_f64).unwrap_or(0.0);
                live.insert(to.into(), Value::from(mbps_to_bps(mbps)));
            }
        }
        if let Some(t) = live.remove("time_remaining")
            && let Some(d) = t.get("duration")
        {
            let secs = d.get("secs").and_then(Value::as_u64).unwrap_or(0);
            let nanos = d.get("nanos").and_then(Value::as_u64).unwrap_or(0);
            live.insert(
                "eta_ms".into(),
                Value::from(secs * 1000 + nanos / 1_000_000),
            );
        }
        if let Some(snap) = live.get_mut("snapshot").and_then(Value::as_object_mut) {
            for k in DROP_SNAPSHOT {
                snap.remove(*k);
            }
            if let Some(ps) = snap.get_mut("peer_stats").and_then(Value::as_object_mut) {
                for k in DROP_PEER_STATS {
                    ps.remove(*k);
                }
            }
        }
    }
    v
}

/// The server's speeds are `bytes_per_second / 2^20`, so this is exact.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn mbps_to_bps(mbps: f64) -> u64 {
    (mbps * 1_048_576.0).round().max(0.0) as u64
}

pub fn row_id(row: &Value) -> Option<u64> {
    row.get("id").and_then(Value::as_u64)
}

/// The whole list at one point in time, in the server's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FeedState {
    pub rows: Vec<Value>,
}

impl FeedState {
    /// From a `TorrentListResponse` serialized to JSON (`{"torrents":[..]}`).
    pub fn from_list_value(list: Value) -> Self {
        let rows = match list {
            Value::Object(mut m) => match m.remove("torrents") {
                Some(Value::Array(a)) => a,
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        Self {
            rows: rows
                .into_iter()
                .map(lean_row)
                .filter(|r| row_id(r).is_some())
                .collect(),
        }
    }

    pub fn ids(&self) -> Vec<u64> {
        self.rows.iter().filter_map(row_id).collect()
    }
}

/// RFC 7386 merge patch turning `old` into `new`, or `None` if they're equal.
/// Assumes neither contains null object members (lean rows don't).
pub fn diff_value(old: &Value, new: &Value) -> Option<Value> {
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            let mut patch = Map::new();
            for (k, nv) in n {
                match o.get(k) {
                    None => {
                        patch.insert(k.clone(), nv.clone());
                    }
                    Some(ov) => {
                        if let Some(p) = diff_value(ov, nv) {
                            patch.insert(k.clone(), p);
                        }
                    }
                }
            }
            for k in o.keys() {
                if !n.contains_key(k) {
                    patch.insert(k.clone(), Value::Null);
                }
            }
            (!patch.is_empty()).then_some(Value::Object(patch))
        }
        _ if old == new => None,
        _ => Some(new.clone()),
    }
}

/// Applies an RFC 7386 merge patch.
pub fn apply_patch(target: &mut Value, patch: &Value) {
    let Value::Object(p) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let t = target.as_object_mut().expect("object");
    for (k, v) in p {
        if v.is_null() {
            t.remove(k);
        } else if v.is_object() {
            apply_patch(t.entry(k.clone()).or_insert(Value::Null), v);
        } else {
            t.insert(k.clone(), v.clone());
        }
    }
}

pub fn snapshot_msg(epoch: &str, seq: u64, st: &FeedState) -> Value {
    serde_json::json!({
        "type": "snapshot",
        "proto": PROTO,
        "epoch": epoch,
        "seq": seq,
        "torrents": st.rows,
    })
}

/// Delta from `old` (at `base`) to `new` (at `seq`).
pub fn delta_msg(epoch: &str, base: u64, seq: u64, old: &FeedState, new: &FeedState) -> Value {
    let old_by_id: std::collections::HashMap<u64, &Value> = old
        .rows
        .iter()
        .filter_map(|r| Some((row_id(r)?, r)))
        .collect();
    let new_ids: std::collections::HashSet<u64> = new.ids().into_iter().collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for row in &new.rows {
        let Some(id) = row_id(row) else { continue };
        match old_by_id.get(&id) {
            None => added.push(row.clone()),
            Some(o) => {
                if let Some(Value::Object(mut p)) = diff_value(o, row) {
                    p.insert("id".into(), Value::from(id));
                    changed.push(Value::Object(p));
                }
            }
        }
    }
    let removed: Vec<u64> = old
        .ids()
        .into_iter()
        .filter(|id| !new_ids.contains(id))
        .collect();
    // The order a client gets without an explicit `order`.
    let mut implied: Vec<u64> = old
        .ids()
        .into_iter()
        .filter(|id| new_ids.contains(id))
        .collect();
    implied.extend(added.iter().filter_map(row_id));
    let new_order = new.ids();

    let mut m = Map::new();
    m.insert("type".into(), "delta".into());
    m.insert("proto".into(), PROTO.into());
    m.insert("epoch".into(), epoch.into());
    m.insert("base".into(), base.into());
    m.insert("seq".into(), seq.into());
    if !added.is_empty() {
        m.insert("added".into(), Value::Array(added));
    }
    if !removed.is_empty() {
        m.insert("removed".into(), removed.into());
    }
    if !changed.is_empty() {
        m.insert("changed".into(), Value::Array(changed));
    }
    if implied != new_order {
        m.insert("order".into(), new_order.into());
    }
    Value::Object(m)
}

/// Why a message couldn't be applied; the client then resyncs.
#[derive(Debug, PartialEq, Eq)]
pub enum ApplyError {
    /// Different epoch, or `base` isn't the client's `seq`.
    Gap,
    /// Unknown type / protocol, or malformed.
    Invalid,
}

/// A client's copy of the feed. The Rust reference for the web UI and GPUI copies;
/// used in tests here.
#[derive(Clone, Debug, Default)]
pub struct FeedClient {
    pub epoch: Option<String>,
    pub seq: u64,
    pub state: FeedState,
}

impl FeedClient {
    /// Applies a snapshot or delta (heartbeats and pongs are ignored).
    pub fn apply(&mut self, msg: &Value) -> Result<(), ApplyError> {
        let ty = msg
            .get("type")
            .and_then(Value::as_str)
            .ok_or(ApplyError::Invalid)?;
        if matches!(ty, "heartbeat" | "pong") {
            return Ok(());
        }
        if msg.get("proto").and_then(Value::as_u64) != Some(u64::from(PROTO)) {
            return Err(ApplyError::Invalid);
        }
        let epoch = msg
            .get("epoch")
            .and_then(Value::as_str)
            .ok_or(ApplyError::Invalid)?;
        let seq = msg
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or(ApplyError::Invalid)?;
        match ty {
            "snapshot" => {
                let rows = msg
                    .get("torrents")
                    .and_then(Value::as_array)
                    .ok_or(ApplyError::Invalid)?;
                self.state = FeedState { rows: rows.clone() };
            }
            "delta" => {
                let base = msg
                    .get("base")
                    .and_then(Value::as_u64)
                    .ok_or(ApplyError::Invalid)?;
                if self.epoch.as_deref() != Some(epoch) || base != self.seq {
                    return Err(ApplyError::Gap);
                }
                let list = |k: &str| {
                    msg.get(k)
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                };
                let removed: std::collections::HashSet<u64> =
                    list("removed").iter().filter_map(Value::as_u64).collect();
                let rows = &mut self.state.rows;
                rows.retain(|r| row_id(r).is_some_and(|id| !removed.contains(&id)));
                for p in list("changed") {
                    let id = row_id(&p).ok_or(ApplyError::Invalid)?;
                    let row = rows
                        .iter_mut()
                        .find(|r| row_id(r) == Some(id))
                        .ok_or(ApplyError::Gap)?;
                    apply_patch(row, &p);
                }
                rows.extend(list("added"));
                if let Some(order) = msg.get("order").and_then(Value::as_array) {
                    let pos: std::collections::HashMap<u64, usize> = order
                        .iter()
                        .filter_map(Value::as_u64)
                        .enumerate()
                        .map(|(i, id)| (id, i))
                        .collect();
                    rows.sort_by_key(|r| {
                        row_id(r)
                            .and_then(|id| pos.get(&id).copied())
                            .unwrap_or(usize::MAX)
                    });
                }
            }
            _ => return Err(ApplyError::Invalid),
        }
        self.epoch = Some(epoch.to_owned());
        self.seq = seq;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
