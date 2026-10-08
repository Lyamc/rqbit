//! `GET /stream/torrents`: the torrent list as a snapshot plus deltas (see
//! `crate::list_feed` for the message format).
//!
//! - With `Upgrade: websocket`: a WebSocket. The server sends a snapshot, then a delta
//!   whenever the list changed, checked every `tick_ms` (default 1000, 250..=10000).
//!   The client may send `{"type":"resync"}` (→ snapshot), `{"type":"refresh"}` (→ check
//!   now), `{"type":"tick","ms":N}` and `{"type":"ping"}` (→ `{"type":"pong"}`). The
//!   server pings every 15 s, sends `{"type":"heartbeat"}` after 15 s without other
//!   messages, and closes the socket after 45 s without hearing from the client.
//! - `enc=deflate`: messages are binary frames of one raw-deflate stream (RFC 1951,
//!   one compression context per connection, sync-flushed after every message), each
//!   message a line of JSON ending in `\n`. Browsers decode it with
//!   `DecompressionStream("deflate-raw")`. This is what permessage-deflate would do;
//!   axum's WebSocket (tungstenite) can't negotiate permessage-deflate. Without `enc`,
//!   messages are plain JSON text frames (easy to read with any WebSocket tool).
//! - Without `Upgrade`: the polling fallback. `?since=<seq>&epoch=<epoch>` answers with
//!   a delta from that version, or a snapshot when it's unknown (too old, another epoch
//!   after a restart, or no `since`). Responses are compressed by the HTTP layer.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Query, State};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use super::HttpApi;
use crate::api::{Api, ApiTorrentListOpts};
use crate::list_feed::{FeedState, delta_msg, snapshot_msg};

const HISTORY: usize = 32;
const PING_EVERY: Duration = Duration::from_secs(15);
const HEARTBEAT_AFTER: Duration = Duration::from_secs(15);
const DEAD_AFTER: Duration = Duration::from_secs(45);
const DEFAULT_TICK_MS: u64 = 1000;

/// Shared by all streams and polls: the latest lean list with a version number, plus
/// the last few versions for `?since=`.
pub(crate) struct ListFeedHub {
    epoch: String,
    inner: parking_lot::Mutex<HubInner>,
}

struct HubInner {
    seq: u64,
    state: Arc<FeedState>,
    at: Option<Instant>,
    history: VecDeque<(u64, Arc<FeedState>)>,
}

impl ListFeedHub {
    pub(crate) fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        Self {
            epoch: format!("{:x}", nanos ^ u128::from(rand::random::<u64>())),
            inner: parking_lot::Mutex::new(HubInner {
                seq: 0,
                state: Arc::default(),
                at: None,
                history: VecDeque::new(),
            }),
        }
    }

    /// The current version, rebuilt from the session if the cached one is older than
    /// `max_age`. The version number only moves when the list changed.
    fn current(&self, api: &Api, max_age: Duration) -> (u64, Arc<FeedState>) {
        let mut g = self.inner.lock();
        if g.at.is_some_and(|t| t.elapsed() < max_age) {
            return (g.seq, g.state.clone());
        }
        let list = api.api_torrent_list_ext(ApiTorrentListOpts { with_stats: true });
        let st = FeedState::from_list_value(serde_json::to_value(&list).unwrap_or_default());
        g.at = Some(Instant::now());
        if g.seq == 0 || *g.state != st {
            g.seq += 1;
            g.state = Arc::new(st);
            let entry = (g.seq, g.state.clone());
            g.history.push_back(entry);
            while g.history.len() > HISTORY {
                g.history.pop_front();
            }
        }
        (g.seq, g.state.clone())
    }

    fn at_version(&self, seq: u64) -> Option<Arc<FeedState>> {
        let g = self.inner.lock();
        g.history
            .iter()
            .find(|(s, _)| *s == seq)
            .map(|(_, st)| st.clone())
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct StreamQuery {
    since: Option<u64>,
    epoch: Option<String>,
    tick_ms: Option<u64>,
    enc: Option<String>,
}

pub(crate) async fn h_stream_torrents(
    State(state): State<Arc<HttpApi>>,
    Query(q): Query<StreamQuery>,
    req: axum::extract::Request,
) -> Response {
    let upgrade = req
        .headers()
        .get(http::header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    if !upgrade {
        return axum::Json(poll_answer(&state, &q)).into_response();
    }
    let (mut parts, _) = req.into_parts();
    match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws
            .on_upgrade(move |socket| run_socket(state, socket, q))
            .into_response(),
        Err(e) => e.into_response(),
    }
}

fn poll_answer(state: &HttpApi, q: &StreamQuery) -> Value {
    let hub = &state.list_feed;
    let (seq, cur) = hub.current(&state.api, Duration::from_millis(250));
    match (q.since, q.epoch.as_deref()) {
        (Some(since), Some(e)) if e == hub.epoch => {
            if since == seq {
                delta_msg(&hub.epoch, seq, seq, &cur, &cur)
            } else if let Some(old) = hub.at_version(since) {
                delta_msg(&hub.epoch, since, seq, &old, &cur)
            } else {
                snapshot_msg(&hub.epoch, seq, &cur)
            }
        }
        _ => snapshot_msg(&hub.epoch, seq, &cur),
    }
}

fn clamp_tick(ms: u64) -> Duration {
    Duration::from_millis(ms.clamp(250, 10_000))
}

/// One raw-deflate stream per connection (context kept across messages).
struct Deflater(flate2::Compress);

impl Deflater {
    fn new() -> Self {
        Self(flate2::Compress::new(flate2::Compression::new(6), false))
    }

    fn message(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len() / 3 + 64);
        let mut consumed = 0usize;
        loop {
            if out.len() == out.capacity() {
                out.reserve(out.capacity().max(1024));
            }
            let before = self.0.total_in();
            if self
                .0
                .compress_vec(&input[consumed..], &mut out, flate2::FlushCompress::Sync)
                .is_err()
            {
                break;
            }
            consumed += usize::try_from(self.0.total_in() - before).unwrap_or(0);
            if consumed >= input.len() && out.len() < out.capacity() {
                break;
            }
        }
        out
    }
}

struct Conn {
    state: Arc<HttpApi>,
    tx: futures::stream::SplitSink<WebSocket, Message>,
    deflate: Option<Deflater>,
    sent: Option<(u64, Arc<FeedState>)>,
    last_send: Instant,
}

impl Conn {
    async fn send(&mut self, msg: &Value) -> bool {
        let mut text = msg.to_string();
        let frame = match &mut self.deflate {
            Some(d) => {
                text.push('\n');
                Message::Binary(d.message(text.as_bytes()).into())
            }
            None => Message::Text(text.into()),
        };
        self.last_send = Instant::now();
        self.tx.send(frame).await.is_ok()
    }

    /// Sends a snapshot (first time / resync) or a delta if the list changed.
    async fn update(&mut self, max_age: Duration, snapshot: bool) -> bool {
        let hub = &self.state.list_feed;
        let (seq, cur) = hub.current(&self.state.api, max_age);
        let msg = match &self.sent {
            Some((s, old)) if !snapshot => {
                if *s == seq {
                    return true;
                }
                delta_msg(&hub.epoch, *s, seq, old, &cur)
            }
            _ => snapshot_msg(&hub.epoch, seq, &cur),
        };
        self.sent = Some((seq, cur));
        self.send(&msg).await
    }
}

async fn run_socket(state: Arc<HttpApi>, socket: WebSocket, q: StreamQuery) {
    let (tx, mut rx) = socket.split();
    let mut tick = clamp_tick(q.tick_ms.unwrap_or(DEFAULT_TICK_MS));
    let mut conn = Conn {
        state,
        tx,
        deflate: (q.enc.as_deref() == Some("deflate")).then(Deflater::new),
        sent: None,
        last_send: Instant::now(),
    };
    let max_age = |tick: Duration| tick.min(Duration::from_secs(1)) / 2;
    let mut ticker = tokio::time::interval(tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut pinger = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    let mut heard = Instant::now();
    debug!("list stream connected");
    loop {
        let ok = tokio::select! {
            _ = ticker.tick() => conn.update(max_age(tick), false).await,
            _ = pinger.tick() => {
                if heard.elapsed() > DEAD_AFTER {
                    debug!("list stream: client silent, closing");
                    break;
                }
                let mut ok = conn.tx.send(Message::Ping(Vec::new().into())).await.is_ok();
                if ok && conn.last_send.elapsed() >= HEARTBEAT_AFTER {
                    let hub = &conn.state.list_feed;
                    let seq = conn.sent.as_ref().map(|(s, _)| *s).unwrap_or_default();
                    let hb = serde_json::json!({"type": "heartbeat", "epoch": hub.epoch, "seq": seq});
                    ok = conn.send(&hb).await;
                }
                ok
            }
            m = rx.next() => {
                let Some(Ok(m)) = m else { break };
                heard = Instant::now();
                match m {
                    Message::Text(t) => {
                        let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
                        match v.get("type").and_then(Value::as_str) {
                            Some("resync") => conn.update(max_age(tick), true).await,
                            Some("refresh") => conn.update(Duration::ZERO, false).await,
                            Some("ping") => conn.send(&serde_json::json!({"type": "pong"})).await,
                            Some("tick") => {
                                if let Some(ms) = v.get("ms").and_then(Value::as_u64) {
                                    tick = clamp_tick(ms);
                                    ticker = tokio::time::interval(tick);
                                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                                    ticker.tick().await;
                                }
                                true
                            }
                            _ => true,
                        }
                    }
                    Message::Close(_) => break,
                    _ => true,
                }
            }
        };
        if !ok {
            break;
        }
    }
    debug!("list stream closed");
}

#[cfg(test)]
mod tests {
    use super::Deflater;
    use std::io::Read;

    /// Messages decode one by one from a single raw-deflate stream (what the browser's
    /// `DecompressionStream("deflate-raw")` sees), and later ones reuse the context.
    #[test]
    fn deflate_messages_share_one_stream() {
        let mut d = Deflater::new();
        // The same incompressible-looking text in every message: only the context helps.
        let mut x = 0x2545_f491_u32;
        let pad: String = (0..300)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                char::from(b'a' + (x % 26) as u8)
            })
            .collect();
        let msgs: Vec<String> = (0..5)
            .map(|i| format!("{{\"type\":\"delta\",\"seq\":{i},\"pad\":\"{pad}\"}}\n"))
            .collect();
        let frames: Vec<Vec<u8>> = msgs.iter().map(|m| d.message(m.as_bytes())).collect();
        assert!(
            frames[1].len() * 3 < frames[0].len(),
            "context takeover: {} vs {}",
            frames[1].len(),
            frames[0].len()
        );
        let mut dec = flate2::Decompress::new(false);
        let mut text = String::new();
        for f in &frames {
            let mut out = Vec::with_capacity(4096);
            dec.decompress_vec(f, &mut out, flate2::FlushDecompress::Sync)
                .unwrap();
            text.push_str(std::str::from_utf8(&out).unwrap());
        }
        assert_eq!(text, msgs.concat());
        // And as one continuous stream.
        let all: Vec<u8> = frames.concat();
        let mut s = String::new();
        flate2::read::DeflateDecoder::new(&all[..])
            .read_to_string(&mut s)
            .ok();
        assert_eq!(s, msgs.concat());
    }
}
