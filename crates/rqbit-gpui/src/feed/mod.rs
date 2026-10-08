//! Client side of the server's lean torrent-list feed (`GET /stream/torrents`; the
//! format is described in `librqbit/src/list_feed/mod.rs`). [`FeedClient`] applies
//! snapshots and deltas and turns the lean rows back into the [`TorrentListItem`]s
//! the UI uses; [`transport::TorrentFeed`] gets the messages (WebSocket, else delta
//! polling) and hides which one is in use. Mirrors the web UI's
//! `helper/listFeed.ts` / `helper/torrentFeed.ts`.

pub mod transport;

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::api::TorrentListItem;

pub const FEED_PROTO: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    /// A new list state (snapshot or delta).
    Updated,
    /// Heartbeat / pong: nothing changed.
    Unchanged,
    /// Delta that doesn't continue our state: resync.
    Gap,
    Invalid,
}

/// RFC 7386 merge patch: null removes, objects merge recursively, the rest replaces.
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

fn row_id(v: &Value) -> Option<u64> {
    v.get("id").and_then(Value::as_u64)
}

fn ids(v: Option<&Value>) -> Vec<u64> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

fn rows(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// A client's copy of the feed: rows in the server's order, keyed by id.
#[derive(Default)]
pub struct FeedClient {
    pub epoch: Option<String>,
    pub seq: u64,
    rows: HashMap<u64, Value>,
    order: Vec<u64>,
    /// Expanded rows, rebuilt only for rows that changed.
    expanded: HashMap<u64, TorrentListItem>,
}

impl FeedClient {
    pub fn has_state(&self) -> bool {
        self.epoch.is_some()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn apply(&mut self, msg: &Value) -> Apply {
        let ty = msg.get("type").and_then(Value::as_str);
        if matches!(ty, Some("heartbeat" | "pong")) {
            return Apply::Unchanged;
        }
        let (Some(epoch), Some(seq)) = (
            msg.get("epoch").and_then(Value::as_str),
            msg.get("seq").and_then(Value::as_u64),
        ) else {
            return Apply::Invalid;
        };
        if msg.get("proto").and_then(Value::as_u64) != Some(FEED_PROTO) {
            return Apply::Invalid;
        }
        match ty {
            Some("snapshot") => {
                let Some(list) = msg.get("torrents").and_then(Value::as_array) else {
                    return Apply::Invalid;
                };
                self.rows.clear();
                self.expanded.clear();
                self.order.clear();
                for r in list {
                    if let Some(id) = row_id(r)
                        && self.rows.insert(id, r.clone()).is_none()
                    {
                        self.order.push(id);
                    }
                }
            }
            Some("delta") => {
                if self.epoch.as_deref() != Some(epoch)
                    || msg.get("base").and_then(Value::as_u64) != Some(self.seq)
                {
                    return Apply::Gap;
                }
                let changed = rows(msg.get("changed"));
                if changed
                    .iter()
                    .any(|p| row_id(p).is_none_or(|id| !self.rows.contains_key(&id)))
                {
                    return Apply::Gap;
                }
                let removed = ids(msg.get("removed"));
                for id in &removed {
                    self.rows.remove(id);
                    self.expanded.remove(id);
                }
                if !removed.is_empty() {
                    self.order.retain(|id| !removed.contains(id));
                }
                for p in changed {
                    let id = row_id(p).expect("checked");
                    apply_patch(self.rows.get_mut(&id).expect("checked"), p);
                    self.expanded.remove(&id);
                }
                for r in rows(msg.get("added")) {
                    let Some(id) = row_id(r) else { continue };
                    if self.rows.insert(id, r.clone()).is_none() {
                        self.order.push(id);
                    }
                    self.expanded.remove(&id);
                }
                if msg.get("order").is_some() {
                    let order = ids(msg.get("order"));
                    self.order = order
                        .into_iter()
                        .filter(|id| self.rows.contains_key(id))
                        .collect();
                }
            }
            _ => return Apply::Invalid,
        }
        self.epoch = Some(epoch.to_owned());
        self.seq = seq;
        Apply::Updated
    }

    /// Lean rows in order (tests).
    #[cfg(test)]
    pub fn lean_rows(&self) -> Vec<&Value> {
        self.order.iter().map(|id| &self.rows[id]).collect()
    }

    /// The list as `/torrents?with_stats=true` would give it.
    pub fn torrents(&mut self) -> Vec<TorrentListItem> {
        let mut out = Vec::with_capacity(self.order.len());
        for id in &self.order {
            let t = self
                .expanded
                .entry(*id)
                .or_insert_with(|| expand_row(&self.rows[id]));
            out.push(t.clone());
        }
        out
    }
}

const MIB: u128 = 1_048_576;

/// `{:.2} MiB/s` exactly as the server formats `bps / 2^20` (ties to even).
pub fn format_speed(bps: u64) -> String {
    let n = u128::from(bps) * 100;
    let mut q = n / MIB;
    let r = n % MIB;
    if r > MIB / 2 || (r == MIB / 2 && q % 2 == 1) {
        q += 1;
    }
    format!("{}.{:02} MiB/s", q / 100, q % 100)
}

/// The server's `DurationWithHumanReadable` ("1h 2m", "3m 4s", "5s").
pub fn format_eta(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[allow(clippy::cast_precision_loss)]
fn speed(bps: Option<&Value>) -> Value {
    let b = bps.and_then(Value::as_u64).unwrap_or(0);
    serde_json::json!({"mbps": b as f64 / MIB as f64, "human_readable": format_speed(b)})
}

/// Lean row → list item.
pub fn expand_row(row: &Value) -> TorrentListItem {
    let mut v = row.clone();
    if let Some(live) = v.pointer_mut("/stats/live").and_then(Value::as_object_mut) {
        let down = live.remove("download_bps");
        let up = live.remove("upload_bps");
        live.insert("download_speed".into(), speed(down.as_ref()));
        live.insert("upload_speed".into(), speed(up.as_ref()));
        if let Some(eta) = live.remove("eta_ms").and_then(|v| v.as_u64()) {
            live.insert(
                "time_remaining".into(),
                serde_json::json!({
                    "duration": {"secs": eta / 1000, "nanos": (eta % 1000) * 1_000_000},
                    "human_readable": format_eta(eta),
                }),
            );
        }
    }
    serde_json::from_value(v).unwrap_or_default()
}

/// Raw-deflate decoder for `enc=deflate` (one stream per connection).
pub struct Inflater(flate2::Decompress);

impl Default for Inflater {
    fn default() -> Self {
        Self(flate2::Decompress::new(false))
    }
}

impl Inflater {
    /// One WebSocket message → its text (one or more JSON lines).
    pub fn message(&mut self, input: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(input.len() * 4 + 256);
        let mut consumed = 0usize;
        loop {
            if out.len() == out.capacity() {
                out.reserve(out.capacity());
            }
            let before = self.0.total_in();
            let status = self
                .0
                .decompress_vec(&input[consumed..], &mut out, flate2::FlushDecompress::Sync)
                .map_err(|e| anyhow::anyhow!("bad deflate data: {e}"))?;
            consumed += usize::try_from(self.0.total_in() - before).unwrap_or(0);
            if status == flate2::Status::StreamEnd {
                anyhow::bail!("deflate stream ended");
            }
            if consumed >= input.len() && out.len() < out.capacity() {
                return Ok(out);
            }
        }
    }
}

/// Splits decoded text into messages (JSON lines; plain text frames are one).
pub fn parse_lines(text: &[u8]) -> Vec<anyhow::Result<Value>> {
    text.split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .map(|l| serde_json::from_slice(l).map_err(|e| anyhow::anyhow!("bad feed message: {e}")))
        .collect()
}

#[cfg(test)]
mod tests;
